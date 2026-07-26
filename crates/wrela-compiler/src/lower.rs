//! Lowering (plans/M5.md item B): `sema::typed::TypedProgram` ->
//! `mwir::MwirProgram`, for the decision-2 surface exactly (the M3
//! evaluator's own executable subset — 02-language.md §6.1 arithmetic,
//! structs/enums/arrays by value, match, loops, calls, take-as-clone).
//! `eval::interp`/`eval::value` are read throughout (never modified) as
//! the observational contract this lowering must preserve — every
//! comment below citing "interp.rs"/"value.rs" points at the exact
//! function this code mirrors.
//!
//! ## Shape
//!
//! One pass, `lower_program`, walks `program.fns`/`program.structs`/
//! `program.instantiations` (all three already `BTreeMap`s, so iteration
//! order is deterministic) and produces one `mwir::MwirFn` per fn/method/
//! `init`/instantiation, keyed by the exact string
//! `sema::typed::CalleeKey::spelling()` would produce for it (built here
//! directly via string formatting rather than constructing a `CalleeKey`
//! — `Fn(name).spelling() == name`, `Method(s,m).spelling() ==
//! "{s}.{m}"`, and so on; `lower.rs` only ever needs to *match* an
//! existing `CalleeKey` at a call site, never re-derive one for a
//! declaration). The program's own `@image` fn (`program.image_fn`, if
//! any) is skipped entirely — plans/M5.md decision 2: "an `@image` fn is
//! never lowered, it already ran at comptime". plans/M9.md item H2:
//! `@layout_assert` and comptime-legal bare `@test` are likewise
//! host-only (02 §12.1 / §12.2) and skipped by default;
//! `LowerOpts::emit_comptime_tests` opts the latter back in for
//! `diff-eval`'s evaluator-vs-backend comparison only. plans/M9.md
//! item H3: only call-graph-reachable guest keys are lowered (see
//! `guest_reachable_keys` / `guest_reachable_keys_closure`).
//!
//! A `FnBuilder` (below) owns one fn's own growing `temp_types`/`body`;
//! `Lowerer` owns the one whole-program fact that outlives any single
//! fn's builder (`&TypedProgram` plus the shared `rodata` interning
//! table) so `MwirProgram::rodata` dedupes `Str`/`BStr` literals across
//! every fn, not just within one. A local-variable environment
//! (`LEnv`, `Vec<BTreeMap<String, mwir::Temp>>`) mirrors
//! `eval::value::Env` exactly — one scope per block, pushed/popped
//! around every nested block this file lowers.
//!
//! ## Control flow: a two-pass-free, single-walk backpatching assembler
//!
//! Every jump target is an instruction index into the same flat
//! `Vec<Inst>` (decision 3). Since a structured construct's *start* is
//! known the moment lowering reaches it but its own *end* (the natural
//! target of a `break`/an `if`'s "after" label/...) is only known once
//! everything nested inside has been lowered, every forward jump is
//! emitted with a placeholder target and recorded for a `patch_jump`
//! call once the real target position is known (`FnBuilder::patch_jump`)
//! — an ordinary one-pass backpatching assembler, not a separate
//! resolution phase.
//!
//! `break`/`continue` thread through `LoopCtx` (a stack, one frame per
//! enclosing loop): both record their own placeholder jump's index into
//! `break_fixups`/`continue_fixups`, patched once the loop's own end/
//! increment position is known. `defer_marker` mirrors
//! `interp::exec_for`/`exec_stmt`'s own `loop_marker` exactly — the
//! active-defer-stack depth at loop entry, so `break`/`continue` only
//! ever runs the defers registered *inside* this loop, never an
//! enclosing block's.
//!
//! `defer` itself (`run_defers`, `lower_block`'s own tail) mirrors
//! `interp::exec_block`/`run_defers` just as directly: every block tracks
//! the active-defer stack's depth on entry, and — only on the path that
//! falls off the end of the block *normally* — lowers every defer body
//! registered since then, in reverse registration order, before
//! continuing. `lower_stmt`/`lower_block` return whether control
//! definitely left the block (`Return`/`Break`/`Continue`, or an `if`/
//! `match` every one of whose branches/arms itself definitely diverges)
//! so a block that ends in one of those never also runs its own trailing
//! defer-drain a second time (the diverging statement already ran the
//! exact defers *it* owns responsibility for — `TypedStmtKind::Return`'s
//! own full-stack drain, `Break`/`Continue`'s own from-`loop_marker`
//! drain, mirroring `interp.rs`'s identical split).
//!
//! ## Pattern matching: safe-to-compute-unconditionally sub-tests, but a
//! genuinely short-circuited guard
//!
//! A pattern's own sub-tests (tag comparison, payload/tuple/array-element
//! projection) are trap-free reads (`mwir::size_of`'s own "tag +
//! max-payload union" layout guarantees a payload read is always
//! in-bounds regardless of which variant is actually live) — so
//! `lower_pattern_test` computes every sub-test *unconditionally* and
//! folds them with `Inst::BoolAnd` (never a real branch) rather than
//! emitting one jump per sub-pattern. A match arm's own *guard*, though,
//! is an arbitrary expression that **can** trap (call a fn, divide,
//! index...) — `interp::exec_stmt`'s own `Match` arm only ever evaluates
//! a guard *after* confirming the pattern matched, so `lower_match`
//! emits a real `JumpIfFalse` for the pattern test before ever lowering
//! the guard, exactly preserving that short-circuit (documented again at
//! `lower_match` itself, since getting this wrong would be an
//! observable, silent divergence from the evaluator, not just a missed
//! optimization).
//!
//! ## Fail-closed set (plans/M5.md decision 2)
//!
//! Everything below returns `LowerError::unimplemented(...)` — never an
//! approximation:
//!
//! - **Closures** (`TypedExprKind::Closure`) **and any indirect call**
//!   (`TypedExprKind::CallValue`, a bare `TypedExprKind::FnRef` used as a
//!   value). `eval::interp`'s own closure support (`Value::Closure`,
//!   `CallValue`'s own dispatch) is a real, if narrow, feature — but
//!   mapping it here would need a first-class function value (a captured-
//!   environment blob plus an indirect-call instruction) that decision
//!   3's own instruction list never names and decision 4's calling
//!   convention never accounts for; the honest call is to fail closed
//!   rather than invent an ABI plans/M5.md's own codegen item never
//!   signed up for.
//! - **The `?` operator** (`TypedExprKind::Try`). Its own early exit runs
//!   every active defer before propagating (`interp::eval_expr`'s `Try`
//!   arm) exactly like `return`/`break`/`continue` — but doing that
//!   correctly from *inside an expression's own lowering* would mean
//!   threading the defer/loop-context state this file otherwise keeps
//!   strictly statement-level all the way through `lower_expr`'s many
//!   call sites. None of this item's required goldens use `?`; recorded
//!   here as a real, disclosed gap rather than a rushed, riskier
//!   implementation.
//! - **A non-literal `assert`/`panic` message.** `interp::render_message`
//!   falls back to Rust's own `{:?}` `Debug` formatting for a non-`Str`
//!   value; reproducing arbitrary `Debug` output in machine code is not
//!   a real lowering. A *literal* string message lowers to a precomputed
//!   fixed `Inst::AssertFail` payload.
//! - **An `|` (or) pattern** (`TypedPatternKind::Or`). Every other
//!   pattern shape's sub-tests are safe to compute unconditionally
//!   (above); an "or" is the one shape where that trick breaks
//!   correctness for its own *bindings* (two alternatives can bind the
//!   same name to two structurally different values, and only the
//!   alternative that actually matched may survive) — doing this right
//!   needs real per-alternative branching this item's required goldens
//!   never exercise; recorded rather than risked.
//! - **Assigning an `InterruptCell` through a nested field/index chain**
//!   (deeper than bare `self.cell = ...`) — the live-driver-state STLR
//!   path only knows a single field offset of `self` (plans/M7.md item G,
//!   decision 17). Nested `InterruptCell` places stay refused by name
//!   rather than silently writing a frame copy. Ordinary nested
//!   field/index assignment (`self.data[i] = v`, `self.a.b = v`) and
//!   nested `mut self` / `mut` argument places are live (plans/M9.md
//!   item MM).
//! - **Indexing a `Bytes` value**, and **an array/`Bytes` length that is
//!   not a literal or a plain module `const` reference** (`eval_array_len`
//!   below) — the evaluator supports both narrowly; this item's own
//!   required coverage does not exercise either, and extending
//!   `Inst::IndexGet`/`IndexSet`'s `len` to a *dynamic* bound is a real
//!   design question for whichever item first needs it.
//! - **A struct/enum/fn/closure/`@image`-decl-valued module `const`**
//!   (`emit_const_value` below) — every *scalar* (and `Str`/`Bytes`/
//!   array-or-tuple-of-scalars) const folds to a literal at its use site
//!   (a const's value is always comptime-fixed by the time `check_typed`
//!   succeeds, so inlining it is exact, not an approximation); an
//!   aggregate-valued const would need the same struct/enum field-type
//!   table `mwir::LayoutCtx` exists for, which this pass deliberately
//!   never threads through (`mwir.rs`'s own module doc explains why).
//! - **`TypedExprKind::Intrinsic`/`PoolName`** — the `@image` builder
//!   surface, reachable only from the one `@image` fn this pass already
//!   skips outright (`eval::legal` is what keeps these two node kinds
//!   out of every *other* fn's body); present only as a defensive,
//!   should-be-unreachable guard.
//!
//! Everything else async/await/send/with/actor-turn/pool-and-group-
//! runtime/f-string-shaped is **not** in the list above because it
//! cannot reach this pass at all: `sema::bodies` already rejects every
//! one of those constructs at `check_typed` time
//! (`sema::bodies::check_expr`'s `Await`/`Send` arms,
//! `Stmt::With`'s own arm, f-strings never producing a typed `Str` node)
//! — there is no typed-tree node shape left for them to arrive here as.
//! **No checkable (`check_typed`-accepted) construct in this item's own
//! required golden family is unlowerable** — the one deliberately
//! constructed err golden below picks a construct that passes
//! `check_typed` but hits this file's own narrower, disclosed boundary
//! (a non-literal `assert` message), precisely because the fully-general
//! fail-closed set above turned out to have no member reachable through
//! an otherwise-ordinary, `check_typed`-accepted program *except* that
//! one — recorded in the session report as the honest finding it is,
//! not papered over with an invented case.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::value::{self, Value};
use crate::mwir::{self, Inst, MwirFn, MwirProgram, Temp};
use crate::sema::bodies::{self, InstKind};
use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TestKind, TypedDeferBody, TypedEnum, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt,
    TypedStmtKind, TypedStruct,
};
use crate::sema::types::{Type, TypeArg};
use crate::syntax::ast::{self, AccessMode, BinOp};

/// The one lowering diagnostic: printed by `bin/wrela.rs` as
/// `error[unimplemented]: <message>`, matching this compiler's existing
/// house style for a not-yet-implemented pipeline stage
/// (`bin/wrela.rs`'s own `"stage `{other}` is not implemented"` line) —
/// the typed tree carries no spans (decision 1, plans/M3.md), so there
/// is no `at L:C` to add here either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerError {
    pub message: String,
}

impl LowerError {
    /// `construct` is the whole clause including its own copula (`"a
    /// closure literal is"`, `"imports are"`-style — mirrors
    /// `sema::unimplemented_at`'s own `subject` convention exactly, one
    /// stage later), so this only ever supplies `"not implemented yet"`.
    fn unimplemented(construct: impl Into<String>) -> LowerError {
        LowerError {
            message: format!("lowering {} not implemented yet", construct.into()),
        }
    }

    /// A complete fail-closed message that already carries its own ending
    /// (an `imported.unresolvable` note ends in "is not supported yet").
    /// Must not go through `unimplemented` — that would print
    /// `lowering \`name\` is declared … is not supported yet not
    /// implemented yet` (plans/M9.md item D3).
    fn named(message: impl Into<String>) -> LowerError {
        LowerError {
            message: message.into(),
        }
    }

    /// A producer-bug guard — should be unreachable for any program
    /// `sema::check_typed` accepted; mirrors `interp.rs`'s own
    /// `"internal error: ..."` abandonment wording exactly (same
    /// intent: a fact the walk needs but the tree/lowering lacks is a
    /// bug here, not a legitimate rejection).
    fn internal(message: impl Into<String>) -> LowerError {
        LowerError {
            message: format!("internal error: {}", message.into()),
        }
    }
}

type LEnv = Vec<BTreeMap<String, Temp>>;

/// Options for `lower_program_with` (plans/M9.md items H2 / H3). Default is
/// production: host-only fns stay out of MWIR; only guest-reachable keys
/// lower.
#[derive(Debug, Clone, Default)]
pub struct LowerOpts {
    /// When true, emit `TestKind::Comptime` fns as guest code. Only
    /// `diff-eval` sets this — 02 §12.2 says a comptime-legal bare
    /// `@test` runs in the build evaluator, but the evaluator-vs-backend
    /// oracle boots those same bodies as guest code to compare tiers.
    /// Production `wrela build` / `wrela test` / dump stages leave this
    /// false.
    pub emit_comptime_tests: bool,
    /// When `Some`, only these keys may lower (plans/M9.md item H3
    /// whole-closure set from `guest_reachable_keys_closure`). When
    /// `None`, compute `guest_reachable_keys` for this program alone
    /// (dump `--stage=mwir`/`asm` path).
    pub only: Option<BTreeSet<String>>,
}

fn is_host_only_comptime_test(program: &TypedProgram, name: &str, opts: &LowerOpts) -> bool {
    if opts.emit_comptime_tests {
        return false;
    }
    program
        .tests
        .iter()
        .any(|t| t.name == name && t.kind == TestKind::Comptime)
}

/// `key` is a CalleeKey spelling (`drive`, `Maker.build`). Image-builder
/// and comptime-`@test` names are always free fns; comparing only the
/// bare member (`build` from `Maker.build`) against `@image fn build`
/// falsely marks imported methods host-only (plans/M9.md item KK).
fn is_host_only_fn(program: &TypedProgram, key: &str, f: &TypedFn, opts: &LowerOpts) -> bool {
    if program.image_fn.as_deref() == Some(key) {
        return true;
    }
    if f.is_layout_assert {
        return true;
    }
    is_host_only_comptime_test(program, key, opts)
}

/// Guest-reachable CalleeKey spellings for one typed program (plans/M9.md
/// item H3). Used by dump-stage lower and as the per-program walk inside
/// `guest_reachable_keys_closure`.
pub fn guest_reachable_keys(program: &TypedProgram, opts: &LowerOpts) -> BTreeSet<String> {
    guest_reachable_keys_over(&[program], opts)
}

/// Whole-build-closure reachable set (plans/M9.md item H3): seeds from
/// every module, callees looked up in every module's local+imported
/// tables. `try_layout_program` passes this via `LowerOpts::only` so a
/// library module with no actors does not emit its entire surface into
/// the merged image.
pub fn guest_reachable_keys_closure(
    programs: &BTreeMap<String, TypedProgram>,
    opts: &LowerOpts,
) -> BTreeSet<String> {
    let progs: Vec<&TypedProgram> = programs.values().collect();
    guest_reachable_keys_over(&progs, opts)
}

/// CalleeKeys force-rooted into every runtime-bearing emit set
/// (plans/M10.md item A2d / decision 583). Hand-asm `Reloc::Call` /
/// `Asm::bl_call_key` targets that no wrela Call ever names — without
/// these seeds they never lower and the reloc fails with "was never
/// codegen'd". Bare names, not `core.runtime.*`.
pub const RUNTIME_FORCE_ROOT_KEYS: &[&str] = &["__wrela_runtime_probe"];

fn guest_reachable_keys_over(programs: &[&TypedProgram], opts: &LowerOpts) -> BTreeSet<String> {
    let mut work: BTreeSet<String> = BTreeSet::new();
    for p in programs {
        seed_entry_points(p, opts, &mut work);
    }
    // plans/M10.md item A2d / decision 583: force-root runtime helpers
    // that exist in some program's fn table (auto-loaded `core.runtime`).
    for key in RUNTIME_FORCE_ROOT_KEYS {
        if programs.iter().any(|p| lookup_typed_fn(p, key).is_some()) {
            work.insert((*key).to_string());
        }
    }
    if work.is_empty() {
        // Dump-stage / no-guest-surface fallback: cannot prove free fns
        // unreachable from a guest that does not exist.
        for p in programs {
            for key in all_candidate_keys(p, opts) {
                work.insert(key);
            }
        }
        return work;
    }
    let mut reachable = BTreeSet::new();
    while let Some(key) = work.pop_first() {
        if !reachable.insert(key.clone()) {
            continue;
        }
        for p in programs {
            if let Some(f) = lookup_typed_fn(p, &key) {
                if is_host_only_fn(p, &key, f, opts) {
                    // Should not be seeded; defensive: do not walk into
                    // host-only bodies.
                    continue;
                }
                collect_callees_from_fn(p, f, &mut work);
            }
        }
    }
    // Drop any host-only keys that slipped into the set (e.g. a Call
    // naming a layout_assert — should not happen, but fail closed to
    // non-emission rather than emitting host code).
    reachable.retain(|key| {
        programs.iter().all(|p| match lookup_typed_fn(p, key) {
            Some(f) => !is_host_only_fn(p, key, f, opts),
            None => true,
        })
    });
    reachable
}

fn seed_entry_points(program: &TypedProgram, opts: &LowerOpts, out: &mut BTreeSet<String>) {
    for t in &program.tests {
        match t.kind {
            TestKind::Runtime | TestKind::Exhaustive => {
                out.insert(t.name.clone());
            }
            TestKind::Comptime if opts.emit_comptime_tests => {
                out.insert(t.name.clone());
            }
            TestKind::Comptime => {}
        }
    }
    // Free top-level async fns are the group-child surface (02 §9.5) and
    // may be started by name from any reachable `with group`. They are
    // also the dump oracle for async lowering. Sync free fns stay
    // call-graph-only (H3's helper-budget / unused-import fixes).
    for (name, f) in program.fns.iter().chain(program.imported.fns.iter()) {
        if f.is_async && !is_host_only_fn(program, name, f, opts) {
            out.insert(name.clone());
        }
    }
    seed_struct_entries(&program.structs, out);
    seed_struct_entries(&program.imported.structs, out);
    // Instantiated actor/driver structs (rare; same rules).
    for (ikey, inst) in program
        .instantiations
        .iter()
        .chain(program.imported.instantiations.iter())
    {
        if let TypedInstantiation::Struct(s) = inst {
            seed_one_struct(ikey, s, out);
        }
    }
}

fn seed_struct_entries(structs: &BTreeMap<String, TypedStruct>, out: &mut BTreeSet<String>) {
    for (sname, s) in structs {
        seed_one_struct(sname, s, out);
    }
}

fn seed_one_struct(key_prefix: &str, s: &TypedStruct, out: &mut BTreeSet<String>) {
    if !(s.is_actor || s.is_driver) {
        return;
    }
    if s.init.is_some() {
        out.insert(format!("{key_prefix}.init"));
    }
    for (member, f) in &s.methods {
        // Driver: every method (ISR conservatism). Actor: pub methods
        // (message shapes) and @task (drivers only, but keep the bit).
        if s.is_driver || f.is_pub || f.is_task {
            out.insert(format!("{key_prefix}.{member}"));
        }
    }
}

fn all_candidate_keys(program: &TypedProgram, opts: &LowerOpts) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (name, f) in &program.fns {
        if !is_host_only_fn(program, name, f, opts) {
            out.insert(name.clone());
        }
    }
    for (name, f) in &program.imported.fns {
        if !is_host_only_fn(program, name, f, opts) {
            out.insert(name.clone());
        }
    }
    add_struct_member_keys(&program.structs, &mut out);
    add_struct_member_keys(&program.imported.structs, &mut out);
    add_enum_member_keys(&program.enums, &mut out);
    add_enum_member_keys(&program.imported.enums, &mut out);
    for (ikey, inst) in program
        .instantiations
        .iter()
        .chain(program.imported.instantiations.iter())
    {
        match inst {
            TypedInstantiation::Fn(f) => {
                if !f.is_layout_assert {
                    out.insert(ikey.clone());
                }
            }
            TypedInstantiation::Struct(s) => add_one_struct_member_keys(ikey, s, &mut out),
            TypedInstantiation::Enum => {}
        }
    }
    out
}

fn add_struct_member_keys(structs: &BTreeMap<String, TypedStruct>, out: &mut BTreeSet<String>) {
    for (sname, s) in structs {
        add_one_struct_member_keys(sname, s, out);
    }
}

fn add_one_struct_member_keys(key_prefix: &str, s: &TypedStruct, out: &mut BTreeSet<String>) {
    for member in s.methods.keys().chain(s.assoc_fns.keys()) {
        out.insert(format!("{key_prefix}.{member}"));
    }
    if s.init.is_some() {
        out.insert(format!("{key_prefix}.init"));
    }
}

fn add_enum_member_keys(enums: &BTreeMap<String, TypedEnum>, out: &mut BTreeSet<String>) {
    for (ename, e) in enums {
        for member in e.methods.keys().chain(e.assoc_fns.keys()) {
            out.insert(format!("{ename}.{member}"));
        }
    }
}

fn lookup_typed_fn<'a>(program: &'a TypedProgram, key: &str) -> Option<&'a TypedFn> {
    if let Some(f) = program
        .fns
        .get(key)
        .or_else(|| program.imported.fns.get(key))
    {
        return Some(f);
    }
    if let Some((owner, member)) = key.split_once('.') {
        if let Some(s) = program
            .structs
            .get(owner)
            .or_else(|| program.imported.structs.get(owner))
        {
            if member == "init" {
                return s.init.as_ref();
            }
            if let Some(f) = s.methods.get(member).or_else(|| s.assoc_fns.get(member)) {
                return Some(f);
            }
        }
        if let Some(e) = program
            .enums
            .get(owner)
            .or_else(|| program.imported.enums.get(owner))
        {
            if let Some(f) = e.methods.get(member).or_else(|| e.assoc_fns.get(member)) {
                return Some(f);
            }
        }
    }
    // Instantiation keys: `fn:name[...]` or `struct:name[...].member`.
    if let Some(inst) = program
        .instantiations
        .get(key)
        .or_else(|| program.imported.instantiations.get(key))
    {
        if let TypedInstantiation::Fn(f) = inst {
            return Some(f);
        }
    }
    if let Some((ikey, member)) = key.rsplit_once('.') {
        if let Some(TypedInstantiation::Struct(s)) = program
            .instantiations
            .get(ikey)
            .or_else(|| program.imported.instantiations.get(ikey))
        {
            if member == "init" {
                return s.init.as_ref();
            }
            return s.methods.get(member).or_else(|| s.assoc_fns.get(member));
        }
    }
    None
}

fn collect_callees_from_fn(program: &TypedProgram, f: &TypedFn, out: &mut BTreeSet<String>) {
    for p in &f.params {
        if let Some(d) = &p.default {
            collect_callees_from_expr(program, d, out);
        }
    }
    collect_callees_from_stmts(program, &f.body, out);
}

fn collect_callees_from_stmts(
    program: &TypedProgram,
    stmts: &[TypedStmt],
    out: &mut BTreeSet<String>,
) {
    for s in stmts {
        collect_callees_from_stmt(program, s, out);
    }
}

fn collect_callees_from_stmt(program: &TypedProgram, stmt: &TypedStmt, out: &mut BTreeSet<String>) {
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => collect_callees_from_expr(program, value, out),
        TypedStmtKind::Assign { target, value } => {
            collect_callees_from_expr(program, target, out);
            collect_callees_from_expr(program, value, out);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            collect_callees_from_expr(program, cond, out);
            collect_callees_from_stmts(program, then_branch, out);
            for e in elifs {
                collect_callees_from_expr(program, &e.cond, out);
                collect_callees_from_stmts(program, &e.body, out);
            }
            if let Some(b) = else_branch {
                collect_callees_from_stmts(program, b, out);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            collect_callees_from_expr(program, scrutinee, out);
            for arm in arms {
                collect_callees_from_pattern(program, &arm.pattern, out);
                if let Some(g) = &arm.guard {
                    collect_callees_from_expr(program, g, out);
                }
                collect_callees_from_stmts(program, &arm.body, out);
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(a, b, _) => {
                    collect_callees_from_expr(program, a, out);
                    collect_callees_from_expr(program, b, out);
                }
                TypedForIter::Expr(e) => collect_callees_from_expr(program, e, out),
            }
            collect_callees_from_stmts(program, body, out);
        }
        TypedStmtKind::While { cond, body } => {
            collect_callees_from_expr(program, cond, out);
            collect_callees_from_stmts(program, body, out);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(Some(e)) => collect_callees_from_expr(program, e, out),
        TypedStmtKind::Return(None) => {}
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            collect_callees_from_expr(program, cond, out);
            if let Some(m) = message {
                collect_callees_from_expr(program, m, out);
            }
        }
        TypedStmtKind::Defer(TypedDeferBody::Expr(e)) => collect_callees_from_expr(program, e, out),
        TypedStmtKind::Defer(TypedDeferBody::Suite(body)) => {
            collect_callees_from_stmts(program, body, out)
        }
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => {
            collect_callees_from_expr(program, e, out)
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                collect_callees_from_expr(program, c, out);
            }
            if let Some(d) = deadline {
                collect_callees_from_expr(program, d, out);
            }
            collect_callees_from_stmts(program, body, out);
        }
    }
}

fn collect_callees_from_pattern(
    program: &TypedProgram,
    pat: &TypedPattern,
    out: &mut BTreeSet<String>,
) {
    match &pat.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => collect_callees_from_expr(program, e, out),
        TypedPatternKind::Take(inner) => collect_callees_from_pattern(program, inner, out),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                collect_callees_from_pattern(program, p, out);
            }
        }
        TypedPatternKind::Tuple(ps) | TypedPatternKind::Array(ps) | TypedPatternKind::Or(ps) => {
            for p in ps {
                collect_callees_from_pattern(program, p, out);
            }
        }
    }
}

fn collect_callees_from_expr(program: &TypedProgram, expr: &TypedExpr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_)
        | TypedExprKind::Static(_)
        | TypedExprKind::PoolName(_) => {}
        TypedExprKind::FnRef(key) | TypedExprKind::GroupChild(key) => {
            out.insert(key.spelling());
        }
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Panic(base) => collect_callees_from_expr(program, base, out),
        TypedExprKind::Index(a, b)
        | TypedExprKind::Binary(_, a, b)
        | TypedExprKind::And(a, b)
        | TypedExprKind::Or(a, b) => {
            collect_callees_from_expr(program, a, out);
            collect_callees_from_expr(program, b, out);
        }
        TypedExprKind::Is(e, p) => {
            collect_callees_from_expr(program, e, out);
            collect_callees_from_pattern(program, p, out);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            out.insert(callee.spelling());
            if let Some(r) = receiver {
                collect_callees_from_expr(program, r, out);
            }
            for (i, a) in args.iter().enumerate() {
                match a {
                    Some(e) => collect_callees_from_expr(program, e, out),
                    None => {
                        // Defaulted arg: walk the callee's stored default.
                        if let Some(f) = lookup_typed_fn(program, &callee.spelling()) {
                            if let Some(d) = f.params.get(i).and_then(|p| p.default.as_ref()) {
                                collect_callees_from_expr(program, d, out);
                            }
                        }
                    }
                }
            }
        }
        TypedExprKind::CallValue(f, args) => {
            collect_callees_from_expr(program, f, out);
            for a in args {
                collect_callees_from_expr(program, a, out);
            }
        }
        TypedExprKind::Try(inner, conv) => {
            collect_callees_from_expr(program, inner, out);
            if let Some(k) = conv {
                out.insert(k.spelling());
            }
        }
        TypedExprKind::OpCall(key, a, b) => {
            out.insert(key.spelling());
            collect_callees_from_expr(program, a, out);
            collect_callees_from_expr(program, b, out);
        }
        TypedExprKind::EnumConstruct { args, .. }
        | TypedExprKind::Tuple(args)
        | TypedExprKind::List(args) => {
            for a in args {
                collect_callees_from_expr(program, a, out);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            crate::sema::typed::TypedClosureBody::Expr(e) => {
                collect_callees_from_expr(program, e, out)
            }
            crate::sema::typed::TypedClosureBody::Suite(stmts) => {
                collect_callees_from_stmts(program, stmts, out)
            }
        },
        TypedExprKind::StructLiteral { name, fields } => {
            for (_, e) in fields {
                collect_callees_from_expr(program, e, out);
            }
            // Omitted field defaults live on the struct; walk them when
            // this construction site is reachable.
            if let Some(s) = program
                .structs
                .get(name)
                .or_else(|| program.imported.structs.get(name))
            {
                let supplied: BTreeSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                for (fname, def) in &s.field_defaults {
                    if !supplied.contains(fname.as_str()) {
                        collect_callees_from_expr(program, def, out);
                    }
                }
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                collect_callees_from_expr(program, r, out);
            }
            for (_, e) in args {
                collect_callees_from_expr(program, e, out);
            }
        }
    }
}

fn env_lookup(env: &LEnv, name: &str) -> Option<Temp> {
    for scope in env.iter().rev() {
        if let Some(t) = scope.get(name) {
            return Some(*t);
        }
    }
    None
}

fn env_insert(env: &mut LEnv, name: String, t: Temp) {
    env.last_mut().expect("at least one scope").insert(name, t);
}

/// Whole-program lowering state that outlives any single fn's own
/// `FnBuilder` — the typed program (read-only) and the shared `rodata`
/// interning table (`MwirProgram::rodata` dedupes across every fn).
struct Lowerer<'p> {
    prog: &'p TypedProgram,
    rodata: Vec<Vec<u8>>,
    rodata_index: BTreeMap<Vec<u8>, usize>,
}

/// One fn's own growing instruction list/temp table, plus a borrow of
/// the shared `Lowerer` state — see this module's own doc comment.
struct FnBuilder<'p, 'l> {
    lw: &'l mut Lowerer<'p>,
    temp_types: Vec<Type>,
    body: Vec<Inst>,
    /// The fn's declared return type — needed by sync `?` (plans/M7.md
    /// item E1) to build the early `Err` return.
    ret: Type,
    /// plans/M7.md item G: when this fn is a struct member, the struct's
    /// own name — `LoadIrqVector` needs the `@driver` that owns the
    /// vector. `None` for free fns.
    owner_struct: Option<String>,
}

impl<'p, 'l> FnBuilder<'p, 'l> {
    fn prog(&self) -> &'p TypedProgram {
        self.lw.prog
    }

    /// Image-declared blk capacity, if this program's `@image` sealed one.
    fn blk_capacity_sectors(&self) -> Option<u64> {
        self.lw.prog.blk_capacity_sectors
    }

    fn fresh(&mut self, ty: Type) -> Temp {
        self.temp_types.push(ty);
        Temp(self.temp_types.len() - 1)
    }

    fn emit(&mut self, inst: Inst) -> usize {
        self.body.push(inst);
        self.body.len() - 1
    }

    fn here(&self) -> usize {
        self.body.len()
    }

    /// Backpatches a previously-emitted `Jump`/`JumpIfFalse`'s own
    /// `target` field — every forward jump in this file is emitted once
    /// (with a placeholder) and patched exactly once, here.
    fn patch_jump(&mut self, idx: usize, target: usize) {
        match &mut self.body[idx] {
            Inst::Jump { target: t } => *t = target,
            Inst::JumpIfFalse { target: t, .. } => *t = target,
            other => panic!("patch_jump: instruction at {idx} is not a jump: {other:?}"),
        }
    }

    /// Interns `bytes` into the shared, whole-program `rodata` table,
    /// deduplicating by exact byte equality — first occurrence wins the
    /// index, matching `MwirProgram::rodata`'s own doc comment.
    fn intern(&mut self, bytes: Vec<u8>) -> usize {
        if let Some(&i) = self.lw.rodata_index.get(&bytes) {
            return i;
        }
        let i = self.lw.rodata.len();
        self.lw.rodata_index.insert(bytes.clone(), i);
        self.lw.rodata.push(bytes);
        i
    }
}

/// One enclosing loop's own backpatch bookkeeping — `defer_marker`
/// mirrors `interp::exec_for`'s own `loop_marker` (the active-defer-stack
/// depth at loop entry, module doc's own "Control flow" section).
struct LoopCtx {
    break_fixups: Vec<usize>,
    continue_fixups: Vec<usize>,
    defer_marker: usize,
}

// --- entry point ------------------------------------------------------

/// `sema::typed::TypedProgram` -> `mwir::MwirProgram` (module doc's own
/// "Shape" section). Every top-level fn, every struct's methods/
/// associated fns/`init`, and every generic instantiation's own fn/
/// struct-methods lowers — `program.image_fn` (if any) is skipped
/// outright.
/// plans/M7.md item H1: the `(layout, register)` pair an `Mmio.read`/
/// `Mmio.write` node names — the receiver's own `Mmio[L]` type argument
/// and the register-name `Str` leaf `sema::bodies::check_mmio_access`
/// put in the node's first argument.
fn mmio_access_names(
    mmio_ty: &Type,
    args: &[(String, TypedExpr)],
) -> Result<(String, String), LowerError> {
    let Type::Named(cap, targs) = &crate::sema::bodies::unwrap_own(mmio_ty.clone()) else {
        return Err(LowerError::internal(
            "an MMIO access whose receiver is not an `Mmio[L]`".to_string(),
        ));
    };
    if cap != "Mmio" {
        return Err(LowerError::internal(
            "an MMIO access whose receiver is not an `Mmio[L]`".to_string(),
        ));
    }
    let Some(crate::sema::types::TypeArg::Type(Type::Named(layout, _))) = targs.first() else {
        return Err(LowerError::internal(
            "an `Mmio[L]` whose layout argument is not a named type".to_string(),
        ));
    };
    let Some((_, reg)) = args.iter().find(|(l, _)| l == "register") else {
        return Err(LowerError::internal(
            "an MMIO access with no register name".to_string(),
        ));
    };
    let TypedExprKind::Str(name) = &reg.kind else {
        return Err(LowerError::internal(
            "an MMIO access whose register name is not a literal".to_string(),
        ));
    };
    Ok((layout.clone(), name.clone()))
}

/// The declared `@offset` of `register` in `layout`, out of
/// `TypedProgram::layouts` — the very table `types::check_layouts`
/// produced and the checker read, never a second computation of it.
fn mmio_register_offset(
    layout: &str,
    register: &str,
    prog: &TypedProgram,
) -> Result<u64, LowerError> {
    let Some(l) = prog.layouts.iter().find(|l| l.name == layout) else {
        // Reachable only for a layout declared in a *different* module of
        // the build closure than the driver that maps it: `check_layouts`
        // runs per module and `TypedProgram::layouts` is this module's
        // own. Named rather than approximated with offset 0.
        return Err(LowerError::unimplemented(&format!(
            "an MMIO access through `Mmio[{layout}]`, whose `@layout(mmio)` declaration lives in \
             a different module than the driver that maps it — the exact-bytes table this \
             lowering reads is per-module. Declaring the layout beside its driver works today; \
             a cross-module one"
        )));
    };
    match crate::sema::types::mmio_register(l, register) {
        Some(r) => Ok(r.offset),
        None => Err(LowerError::internal(format!(
            "`{layout}` declares no register `{register}` (the checker already refused this)"
        ))),
    }
}

/// Declared offset of `field` in a `@layout(runtime)` type — same table
/// `check_layouts` produced (plans/M10.md item A2c).
fn runtime_layout_field_offset(
    layout: &str,
    field: &str,
    prog: &TypedProgram,
) -> Result<u64, LowerError> {
    let Some(l) = prog.layouts.iter().find(|l| l.name == layout) else {
        return Err(LowerError::internal(format!(
            "placed-static field access through `{layout}`, which has no layout table entry"
        )));
    };
    for e in &l.entries {
        if let crate::sema::types::LayoutEntry::Field(f) = e {
            if f.name == field {
                return Ok(f.offset);
            }
        }
    }
    Err(LowerError::internal(format!(
        "`{layout}` declares no field `{field}` (the checker already refused this)"
    )))
}

fn placed_static_addr(prog: &TypedProgram, name: &str) -> Result<u64, LowerError> {
    prog.statics
        .get(name)
        .map(|s| s.addr)
        .ok_or_else(|| LowerError::internal(format!("placed static `{name}` not in TypedProgram")))
}

/// Exact `@layout(dma)` byte size of an `own[P] T` payload (or bare `T`).
/// Used by `prepare_block` for the descriptor length — mwir `size_of` is
/// the frame ABI (8-byte slots), not the device-visible layout.
pub(crate) fn layout_dma_size(ty: &Type, prog: &TypedProgram) -> Option<u64> {
    let name = match ty {
        Type::Own(_, inner) => match inner.as_ref() {
            Type::Named(n, args) if args.is_empty() => n.as_str(),
            _ => return None,
        },
        Type::Named(n, args) if args.is_empty() => n.as_str(),
        _ => return None,
    };
    prog.layouts
        .iter()
        .find(|l| l.name == name && matches!(l.kind, crate::sema::types::LayoutKind::Dma))
        // plans/M10.md item A2b: `None` also covers a layout whose sizing is
        // still deferred, which is the same answer this fn already gives for
        // "no such `@layout(dma)` type" — every caller treats `None` as "no
        // exact size here", never as zero.
        .and_then(|l| l.size)
}

/// plans/M7.md item H2a: lower `reported.checked_le(bound)` to a compare
/// against the bound and a branch that builds `Ok(payload)` or `Err(unit)`.
fn lower_untrusted_checked_le(
    expr: &TypedExpr,
    receiver: &Option<Box<TypedExpr>>,
    type_arg: &Option<Type>,
    args: &[(String, TypedExpr)],
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    let Some(recv) = receiver else {
        return Err(LowerError::internal(
            "`Untrusted.checked_le` with no receiver".to_string(),
        ));
    };
    let Some(payload_ty) = type_arg.clone() else {
        return Err(LowerError::internal(
            "`Untrusted.checked_le` with no payload type".to_string(),
        ));
    };
    let Some((_, bound_expr)) = args.iter().find(|(l, _)| l == "bound") else {
        return Err(LowerError::internal(
            "`Untrusted.checked_le` with no bound argument".to_string(),
        ));
    };
    // Transparent newtype: the receiver's bits *are* the payload.
    let payload = lower_expr(recv, b, env)?;
    let bound = lower_expr(bound_expr, b, env)?;
    let le = b.fresh(Type::Bool);
    b.emit(Inst::Compare {
        dst: le,
        op: BinOp::Le,
        ty: payload_ty.clone(),
        lhs: payload,
        rhs: bound,
    });
    let result = b.fresh(expr.ty.clone());
    let else_fixup = b.emit(Inst::JumpIfFalse {
        cond: le,
        target: usize::MAX,
    });
    b.emit(Inst::MakeEnum {
        dst: result,
        tag: value::RESULT_OK,
        payload: vec![payload],
    });
    let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
    let else_pos = b.here();
    b.patch_jump(else_fixup, else_pos);
    let err_unit = b.fresh(Type::Unit);
    b.emit(Inst::ConstUnit { dst: err_unit });
    b.emit(Inst::MakeEnum {
        dst: result,
        tag: value::RESULT_ERR,
        payload: vec![err_unit],
    });
    let end_pos = b.here();
    b.patch_jump(end_fixup, end_pos);
    Ok(result)
}

pub fn lower_program(program: &TypedProgram) -> Result<MwirProgram, LowerError> {
    lower_program_with(program, &LowerOpts::default())
}

/// Like `lower_program`, with host-only / reachability opts (plans/M9.md
/// items H2 / H3).
pub fn lower_program_with(
    program: &TypedProgram,
    opts: &LowerOpts,
) -> Result<MwirProgram, LowerError> {
    let reachable = match &opts.only {
        Some(set) => set.clone(),
        None => guest_reachable_keys(program, opts),
    };
    let mut lw = Lowerer {
        prog: program,
        rodata: Vec::new(),
        rodata_index: BTreeMap::new(),
    };
    let mut fns: BTreeMap<String, MwirFn> = BTreeMap::new();

    for (name, f) in &program.fns {
        if program.image_fn.as_deref() == Some(name.as_str()) {
            continue;
        }
        // plans/M6.md item D: an `async fn`/method never reaches this
        // path at all — it lowers via `flowwir_lower::lower_program`
        // instead (decision 2's own hard constraint, `flowwir.rs`'s own
        // module doc: "a sync fn never leaves the M5 typed -> mwir path").
        // Before this item, no `TypedProgram` this fn ever saw declared an
        // `is_async` fn without also choking on an `await`/`send`/`with
        // group` construct deeper inside `lower_fn` (the fail-closed
        // `unimplemented` diagnostics those constructs' own match arms
        // already carry) — this skip is what makes a *mixed* sync+async
        // program's own sync half lower cleanly for the first time.
        if f.is_async {
            continue;
        }
        // plans/M9.md item H2: `@layout_assert` is host-only (02 §12.1) —
        // runs after layout in `eval::layout_assert`, never guest code.
        if f.is_layout_assert {
            continue;
        }
        // plans/M9.md item H2: comptime-legal bare `@test` is host-only
        // (02 §12.2) unless `diff-eval` opts back in.
        if is_host_only_comptime_test(program, name, opts) {
            continue;
        }
        // plans/M9.md item H3: call-graph reachability.
        if !reachable.contains(name) {
            continue;
        }
        let mf = lower_fn(f, None, &mut lw)?;
        fns.insert(name.clone(), mf);
    }
    for (sname, s) in &program.structs {
        lower_struct_members(sname, s, &mut lw, &mut fns, &reachable)?;
    }
    for (ename, e) in &program.enums {
        lower_enum_members(ename, e, &mut lw, &mut fns, &reachable)?;
    }
    for (ikey, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => {
                if f.is_async || f.is_layout_assert || !reachable.contains(ikey) {
                    continue;
                }
                let mf = lower_fn(f, None, &mut lw)?;
                fns.insert(ikey.clone(), mf);
            }
            TypedInstantiation::Struct(s) => {
                lower_struct_members(ikey, s, &mut lw, &mut fns, &reachable)?;
            }
            TypedInstantiation::Enum => {}
        }
    }
    // plans/M9.md item EE / decision 90: emit imported fns/methods under
    // the *local* spelling the typed tree already uses (decision 9), so a
    // Call keyed `Duo.sum` / `twice` resolves in this module's own MWIR
    // without merging the exporter's tables into `prog.fns`/`structs`
    // (decision 13 rejected that — it would emit once per importer when
    // `try_layout_program` lowers every module). Walking `imported` keeps
    // the declaration tables honest ("what this module declares") and
    // still emits each *Call key* once in this program. An unaliased name
    // that also exists in the exporter's own lower may collide at
    // `merge_mwir_programs` last-wins — that collision is already
    // disclosed there. plans/M9.md item H3: only reachable imports.
    for (name, f) in &program.imported.fns {
        if f.is_async
            || f.is_layout_assert
            || is_host_only_comptime_test(program, name, opts)
            || fns.contains_key(name)
            || !reachable.contains(name)
        {
            continue;
        }
        let mf = lower_fn(f, None, &mut lw)?;
        fns.insert(name.clone(), mf);
    }
    for (sname, s) in &program.imported.structs {
        lower_struct_members(sname, s, &mut lw, &mut fns, &reachable)?;
    }
    for (ename, e) in &program.imported.enums {
        lower_enum_members(ename, e, &mut lw, &mut fns, &reachable)?;
    }
    for (ikey, inst) in &program.imported.instantiations {
        if fns.contains_key(ikey) {
            continue;
        }
        match inst {
            TypedInstantiation::Fn(f) => {
                if f.is_async || f.is_layout_assert || !reachable.contains(ikey) {
                    continue;
                }
                let mf = lower_fn(f, None, &mut lw)?;
                fns.insert(ikey.clone(), mf);
            }
            TypedInstantiation::Struct(s) => {
                lower_struct_members(ikey, s, &mut lw, &mut fns, &reachable)?;
            }
            TypedInstantiation::Enum => {}
        }
    }
    Ok(MwirProgram {
        fns,
        rodata: lw.rodata,
    })
}

/// Lowers one struct's own methods/associated fns/`init`, keyed
/// `"{key_prefix}.{member}"` — byte-identical to what
/// `CalleeKey::Method`/`CalleeKey::MethodInstance::spelling()` would
/// produce for `key_prefix` = the struct's own plain name or its own
/// instantiation key, respectively (this fn's one caller passes whichever
/// applies — the two cases share this one body, decision: no separate
/// "instantiated struct" lowering path).
fn lower_struct_members(
    key_prefix: &str,
    s: &TypedStruct,
    lw: &mut Lowerer,
    fns: &mut BTreeMap<String, MwirFn>,
    reachable: &BTreeSet<String>,
) -> Result<(), LowerError> {
    let owner = Some(key_prefix.to_string());
    for (member, f) in &s.methods {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    for (member, f) in &s.assoc_fns {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    if let Some(f) = &s.init {
        let key = format!("{key_prefix}.init");
        if !f.is_async && reachable.contains(&key) {
            fns.insert(key, lower_fn(f, owner, lw)?);
        }
    }
    Ok(())
}

/// plans/M9.md item B2: same keying as `lower_struct_members` —
/// `"{enum}.{member}"` matches `CalleeKey::Method::spelling()`.
fn lower_enum_members(
    key_prefix: &str,
    e: &crate::sema::typed::TypedEnum,
    lw: &mut Lowerer,
    fns: &mut BTreeMap<String, MwirFn>,
    reachable: &BTreeSet<String>,
) -> Result<(), LowerError> {
    let owner = Some(key_prefix.to_string());
    for (member, f) in &e.methods {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    for (member, f) in &e.assoc_fns {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    Ok(())
}

fn lower_fn(
    f: &TypedFn,
    owner_struct: Option<String>,
    lw: &mut Lowerer,
) -> Result<MwirFn, LowerError> {
    let mut b = FnBuilder {
        lw,
        temp_types: Vec::new(),
        body: Vec::new(),
        ret: f.ret.clone(),
        owner_struct,
    };
    let mut env: LEnv = vec![BTreeMap::new()];
    let receiver = match &f.receiver {
        Some((mode, ty)) => {
            let t = b.fresh(ty.clone());
            env_insert(&mut env, "self".to_string(), t);
            Some((t, *mode))
        }
        None => None,
    };
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        let t = b.fresh(p.ty.clone());
        env_insert(&mut env, p.name.clone(), t);
        params.push((t, p.mode));
    }
    let mut defers: Vec<&TypedDeferBody> = Vec::new();
    let mut loops: Vec<LoopCtx> = Vec::new();
    lower_block(&f.body, &mut b, &mut env, &mut defers, &mut loops)?;
    // Always append a trailing bare `Return` (module doc: dead code when
    // every path already diverged, needed when the body legally falls
    // off its own end — one uniform tail rather than special-casing
    // which case this is).
    b.emit(Inst::Return { value: None });
    Ok(MwirFn {
        receiver,
        params,
        ret: f.ret.clone(),
        temp_types: b.temp_types,
        body: b.body,
    })
}

// --- statements ---------------------------------------------------------

/// Lowers one block's own statements; returns whether control definitely
/// left the block (module doc's own "Control flow" section) — the
/// trailing defer-drain only ever runs on the *non*-diverging path,
/// mirroring `interp::exec_block` exactly (that fn's own `?`-based
/// short-circuit never reaches its own trailing `run_defers` either, once
/// any statement propagates an early exit).
fn lower_block<'a>(
    stmts: &'a [TypedStmt],
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    let start = defers.len();
    let mut diverged = false;
    for s in stmts {
        if lower_stmt(s, b, env, defers, loops)? {
            diverged = true;
            break;
        }
    }
    if !diverged {
        let active: Vec<&TypedDeferBody> = defers[start..].to_vec();
        run_defers(&active, b, env)?;
    }
    defers.truncate(start);
    Ok(diverged)
}

/// Lowers every defer body in `active`, in reverse (registration order),
/// using the *current* variable environment (not a fresh one — a defer
/// body can reference enclosing locals/`self`, `interp::run_defers`'s own
/// behavior) but a fresh, empty defer/loop context of its own (a defer
/// body can neither `await`/`?` nor legally `break`/`continue` out of an
/// enclosing loop, `sema::bodies::scan_defer_forbidden`'s own guard, so
/// nothing here should ever need either).
fn run_defers(
    active: &[&TypedDeferBody],
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(), LowerError> {
    for d in active.iter().rev() {
        let mut inner_defers: Vec<&TypedDeferBody> = Vec::new();
        let mut inner_loops: Vec<LoopCtx> = Vec::new();
        match d {
            TypedDeferBody::Expr(e) => {
                lower_expr(e, b, env)?;
            }
            TypedDeferBody::Suite(stmts) => {
                lower_block(stmts, b, env, &mut inner_defers, &mut inner_loops)?;
            }
        }
    }
    Ok(())
}

fn lower_stmt<'a>(
    stmt: &'a TypedStmt,
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty, value } => {
            let v = lower_expr(value, b, env)?;
            let t = b.fresh(ty.clone());
            b.emit(Inst::Copy { dst: t, src: v });
            env_insert(env, name.clone(), t);
            Ok(false)
        }
        TypedStmtKind::Assign { target, value } => {
            let v = lower_expr(value, b, env)?;
            lower_place_write(target, v, b, env)?;
            Ok(false)
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => lower_if(cond, then_branch, elifs, else_branch, b, env, defers, loops),
        TypedStmtKind::Match { scrutinee, arms } => {
            lower_match(scrutinee, arms, b, env, defers, loops)
        }
        TypedStmtKind::While { cond, body } => {
            lower_while(cond, body, b, env, defers, loops)?;
            Ok(false)
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            iter,
            body,
            ..
        } => {
            lower_for(name, elem_ty, iter, body, b, env, defers, loops)?;
            Ok(false)
        }
        TypedStmtKind::Break => {
            let marker = loops
                .last()
                .ok_or_else(|| LowerError::internal("`break` outside a loop"))?
                .defer_marker;
            let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
            run_defers(&active, b, env)?;
            let idx = b.emit(Inst::Jump { target: usize::MAX });
            loops
                .last_mut()
                .expect("checked above")
                .break_fixups
                .push(idx);
            Ok(true)
        }
        TypedStmtKind::Continue => {
            let marker = loops
                .last()
                .ok_or_else(|| LowerError::internal("`continue` outside a loop"))?
                .defer_marker;
            let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
            run_defers(&active, b, env)?;
            let idx = b.emit(Inst::Jump { target: usize::MAX });
            loops
                .last_mut()
                .expect("checked above")
                .continue_fixups
                .push(idx);
            Ok(true)
        }
        TypedStmtKind::Pass => Ok(false),
        TypedStmtKind::Return(value) => {
            let v = match value {
                Some(e) => Some(lower_expr(e, b, env)?),
                None => None,
            };
            let active: Vec<&TypedDeferBody> = defers[..].to_vec();
            run_defers(&active, b, env)?;
            b.emit(Inst::Return { value: v });
            Ok(true)
        }
        TypedStmtKind::Assert { cond, message } => {
            let c = lower_expr(cond, b, env)?;
            let fail_fixup = b.emit(Inst::JumpIfFalse {
                cond: c,
                target: usize::MAX,
            });
            let after_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let fail_pos = b.here();
            b.patch_jump(fail_fixup, fail_pos);
            let msg = match message {
                Some(m) => Some(assert_message_text(m)?),
                None => None,
            };
            b.emit(Inst::AssertFail { message: msg });
            let after_pos = b.here();
            b.patch_jump(after_fixup, after_pos);
            Ok(false)
        }
        // Checked exactly once, unconditionally, by `eval::check_comptime_asserts`
        // before this fn's own body is ever lowered — a no-op here,
        // exactly like `interp::exec_stmt`'s own identical arm (its own
        // doc comment explains why in full).
        TypedStmtKind::ComptimeAssert { .. } => Ok(false),
        TypedStmtKind::Defer(body) => {
            defers.push(body);
            Ok(false)
        }
        TypedStmtKind::ExprStmt(e) => {
            lower_expr(e, b, env)?;
            Ok(false)
        }
        // Plans/M6.md item A: sema now types `with group(...)` (real
        // node, no longer fail-closed at the sema layer) but this pass
        // (M5's sync-fn-only lowering) is not that lowering — item B
        // (FlowWir) owns state-machine lowering for every async/actor
        // construct; fail closed, named, here rather than mis-lowering a
        // suspension point as if it were straight-line sync code.
        TypedStmtKind::WithGroup { .. } => Err(LowerError::unimplemented(
            "`with group` (FlowWir state machines, plans/M6.md item B) is",
        )),
        // plans/M6.md item G: `send` requires an `async fn` context
        // (`bodies::check_send`), and this pass only ever lowers sync
        // fns — unreachable in practice, fail closed rather than
        // mis-lowering an enqueue as straight-line sync code.
        TypedStmtKind::BareSend { .. } => Err(LowerError::unimplemented(
            "a bare `send` statement (FlowWir state machines, plans/M6.md item B) is",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_if<'a>(
    cond: &'a TypedExpr,
    then_branch: &'a [TypedStmt],
    elifs: &'a [crate::sema::typed::TypedElif],
    else_branch: &'a Option<Vec<TypedStmt>>,
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    let mut end_fixups = Vec::new();
    let mut all_diverge = true;

    let c = lower_expr(cond, b, env)?;
    let mut next_fixup = b.emit(Inst::JumpIfFalse {
        cond: c,
        target: usize::MAX,
    });
    env.push(BTreeMap::new());
    let d = lower_block(then_branch, b, env, defers, loops)?;
    env.pop();
    if !d {
        all_diverge = false;
    }
    end_fixups.push(b.emit(Inst::Jump { target: usize::MAX }));
    let mut pos = b.here();
    b.patch_jump(next_fixup, pos);

    for elif in elifs {
        let c2 = lower_expr(&elif.cond, b, env)?;
        next_fixup = b.emit(Inst::JumpIfFalse {
            cond: c2,
            target: usize::MAX,
        });
        env.push(BTreeMap::new());
        let d2 = lower_block(&elif.body, b, env, defers, loops)?;
        env.pop();
        if !d2 {
            all_diverge = false;
        }
        end_fixups.push(b.emit(Inst::Jump { target: usize::MAX }));
        pos = b.here();
        b.patch_jump(next_fixup, pos);
    }

    match else_branch {
        Some(eb) => {
            env.push(BTreeMap::new());
            let de = lower_block(eb, b, env, defers, loops)?;
            env.pop();
            if !de {
                all_diverge = false;
            }
        }
        None => all_diverge = false,
    }

    let end_pos = b.here();
    for idx in end_fixups {
        b.patch_jump(idx, end_pos);
    }
    Ok(all_diverge)
}

fn lower_while<'a>(
    cond: &'a TypedExpr,
    body: &'a [TypedStmt],
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), LowerError> {
    loops.push(LoopCtx {
        break_fixups: Vec::new(),
        continue_fixups: Vec::new(),
        defer_marker: defers.len(),
    });
    let cond_pos = b.here();
    let c = lower_expr(cond, b, env)?;
    let end_fixup = b.emit(Inst::JumpIfFalse {
        cond: c,
        target: usize::MAX,
    });
    env.push(BTreeMap::new());
    lower_block(body, b, env, defers, loops)?;
    env.pop();
    b.emit(Inst::Jump { target: cond_pos });
    let end_pos = b.here();
    b.patch_jump(end_fixup, end_pos);
    let ctx = loops.pop().expect("pushed above");
    for idx in ctx.break_fixups {
        b.patch_jump(idx, end_pos);
    }
    for idx in ctx.continue_fixups {
        b.patch_jump(idx, cond_pos);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_for<'a>(
    name: &str,
    elem_ty: &Type,
    iter: &'a TypedForIter,
    body: &'a [TypedStmt],
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), LowerError> {
    loops.push(LoopCtx {
        break_fixups: Vec::new(),
        continue_fixups: Vec::new(),
        defer_marker: defers.len(),
    });
    match iter {
        TypedForIter::Range(from, to, inclusive) => {
            let from_t = lower_expr(from, b, env)?;
            let to_t = lower_expr(to, b, env)?;
            let i_temp = b.fresh(elem_ty.clone());
            b.emit(Inst::Copy {
                dst: i_temp,
                src: from_t,
            });
            let cond_pos = b.here();
            let cmp_op = if *inclusive { BinOp::Le } else { BinOp::Lt };
            let cond_t = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: cond_t,
                op: cmp_op,
                ty: elem_ty.clone(),
                lhs: i_temp,
                rhs: to_t,
            });
            let end_fixup = b.emit(Inst::JumpIfFalse {
                cond: cond_t,
                target: usize::MAX,
            });
            env.push(BTreeMap::new());
            env_insert(env, name.to_string(), i_temp);
            lower_block(body, b, env, defers, loops)?;
            env.pop();
            let incr_pos = b.here();
            let one_t = b.fresh(elem_ty.clone());
            b.emit(Inst::ConstInt {
                dst: one_t,
                ty: elem_ty.clone(),
                value: 1,
            });
            let next_t = b.fresh(elem_ty.clone());
            b.emit(Inst::ArithWrapping {
                dst: next_t,
                op: BinOp::AddW,
                ty: elem_ty.clone(),
                lhs: i_temp,
                rhs: one_t,
            });
            b.emit(Inst::Copy {
                dst: i_temp,
                src: next_t,
            });
            b.emit(Inst::Jump { target: cond_pos });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            let ctx = loops.pop().expect("pushed above");
            for idx in ctx.break_fixups {
                b.patch_jump(idx, end_pos);
            }
            for idx in ctx.continue_fixups {
                b.patch_jump(idx, incr_pos);
            }
        }
        TypedForIter::Expr(arr) => {
            let arr_t = lower_expr(arr, b, env)?;
            let len = eval_array_len(&arr.ty)?;
            let idx_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: idx_t,
                ty: Type::Usize,
                value: 0,
            });
            let len_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: len_t,
                ty: Type::Usize,
                value: len as i128,
            });
            let cond_pos = b.here();
            let cond_t = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: cond_t,
                op: BinOp::Lt,
                ty: Type::Usize,
                lhs: idx_t,
                rhs: len_t,
            });
            let end_fixup = b.emit(Inst::JumpIfFalse {
                cond: cond_t,
                target: usize::MAX,
            });
            let elem_t = b.fresh(elem_ty.clone());
            b.emit(Inst::IndexGet {
                dst: elem_t,
                base: arr_t,
                index: idx_t,
                len,
            });
            env.push(BTreeMap::new());
            env_insert(env, name.to_string(), elem_t);
            lower_block(body, b, env, defers, loops)?;
            env.pop();
            let incr_pos = b.here();
            let one_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: one_t,
                ty: Type::Usize,
                value: 1,
            });
            let next_t = b.fresh(Type::Usize);
            b.emit(Inst::ArithWrapping {
                dst: next_t,
                op: BinOp::AddW,
                ty: Type::Usize,
                lhs: idx_t,
                rhs: one_t,
            });
            b.emit(Inst::Copy {
                dst: idx_t,
                src: next_t,
            });
            b.emit(Inst::Jump { target: cond_pos });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            let ctx = loops.pop().expect("pushed above");
            for idx in ctx.break_fixups {
                b.patch_jump(idx, end_pos);
            }
            for idx in ctx.continue_fixups {
                b.patch_jump(idx, incr_pos);
            }
        }
    }
    Ok(())
}

/// A literal-string assert/panic message, decoded — fails closed on
/// anything else (module doc's own fail-closed enumeration).
fn assert_message_text(e: &TypedExpr) -> Result<String, LowerError> {
    if let TypedExprKind::Str(text) = &e.kind {
        Ok(String::from_utf8_lossy(&value::decode_str(text)).into_owned())
    } else {
        Err(LowerError::unimplemented(
            "a non-literal `assert`/`panic` message is",
        ))
    }
}

// --- match/pattern lowering ------------------------------------------------

/// `interp::exec_stmt`'s own `Match` arm, lowered: tests each arm in
/// source order, only evaluating a guard once the pattern itself matched
/// (module doc's own "Pattern matching" section explains why this one
/// piece needs a real branch rather than the unconditional-sub-test
/// trick `lower_pattern_test` otherwise uses throughout). The trailing
/// `AssertFail` mirrors `interp::exec_stmt`'s own defensive "no arm
/// matched" line verbatim — exhaustiveness already proved it
/// unreachable, kept anyway for parity (never a default arm: no case
/// here ever changes *which* arm is selected).
fn lower_match<'a>(
    scrutinee: &'a TypedExpr,
    arms: &'a [crate::sema::typed::TypedMatchArm],
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    let sv = lower_expr(scrutinee, b, env)?;
    let mut end_fixups = Vec::new();
    let mut all_diverge = true;

    for arm in arms {
        let mut fail_fixups: Vec<usize> = Vec::new();
        let mut bindings = BTreeMap::new();
        collect_pattern_bindings(&arm.pattern, &mut bindings, b);
        let test = lower_pattern_test(&arm.pattern, sv, &bindings, b)?;
        fail_fixups.push(b.emit(Inst::JumpIfFalse {
            cond: test,
            target: usize::MAX,
        }));
        env.push(bindings);
        if let Some(guard) = &arm.guard {
            let g = lower_expr(guard, b, env)?;
            fail_fixups.push(b.emit(Inst::JumpIfFalse {
                cond: g,
                target: usize::MAX,
            }));
        }
        let d = lower_block(&arm.body, b, env, defers, loops)?;
        env.pop();
        if !d {
            all_diverge = false;
        }
        end_fixups.push(b.emit(Inst::Jump { target: usize::MAX }));
        let next_arm_pos = b.here();
        for idx in fail_fixups {
            b.patch_jump(idx, next_arm_pos);
        }
    }
    b.emit(Inst::AssertFail {
        message: Some(
            "match: no arm matched (exhaustiveness already proved this cannot happen)".to_string(),
        ),
    });
    let match_end = b.here();
    for idx in end_fixups {
        b.patch_jump(idx, match_end);
    }
    Ok(all_diverge)
}

/// Pre-allocates one temp per name a pattern binds (recursing through
/// `Take`/`Variant`/`Tuple`/`Array`, mirroring `sema::matches`'s own
/// tree walk) — called once per arm, before `lower_pattern_test`, so a
/// `Binding` leaf always has a destination temp ready to copy into.
fn collect_pattern_bindings(
    pat: &TypedPattern,
    out: &mut BTreeMap<String, Temp>,
    b: &mut FnBuilder,
) {
    match &pat.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            let t = b.fresh(pat.ty.clone());
            out.insert(name.clone(), t);
        }
        TypedPatternKind::Take(inner) => collect_pattern_bindings(inner, out, b),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                collect_pattern_bindings(p, out, b);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for p in items {
                collect_pattern_bindings(p, out, b);
            }
        }
        TypedPatternKind::Or(_) => {}
    }
}

/// Tests `pattern` against the already-lowered `value` temp, writing
/// every binding it introduces into the pre-allocated `bindings` table
/// (via `Inst::Copy`) along the way; returns the `bool` temp holding the
/// overall test result. Every sub-test here is trap-free (module doc's
/// own "Pattern matching" section) so nested tests fold with
/// `Inst::BoolAnd` rather than branching.
fn lower_pattern_test(
    pattern: &TypedPattern,
    value: Temp,
    bindings: &BTreeMap<String, Temp>,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    match &pattern.kind {
        TypedPatternKind::Wildcard => {
            let t = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool {
                dst: t,
                value: true,
            });
            Ok(t)
        }
        TypedPatternKind::Binding(name) => {
            let dst = *bindings
                .get(name)
                .expect("collect_pattern_bindings pre-allocated every binding name");
            b.emit(Inst::Copy { dst, src: value });
            let t = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool {
                dst: t,
                value: true,
            });
            Ok(t)
        }
        TypedPatternKind::Take(inner) => lower_pattern_test(inner, value, bindings, b),
        TypedPatternKind::Literal(lit) => {
            let mut scratch: LEnv = vec![BTreeMap::new()];
            let lit_temp = lower_expr(lit, b, &mut scratch)?;
            let t = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: t,
                op: BinOp::Eq,
                ty: pattern.ty.clone(),
                lhs: value,
                rhs: lit_temp,
            });
            Ok(t)
        }
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } => {
            let want = variant_index(b.prog(), enum_name, variant)?;
            let tag_t = b.fresh(Type::U64);
            b.emit(Inst::EnumTag {
                dst: tag_t,
                src: value,
            });
            let want_t = b.fresh(Type::U64);
            b.emit(Inst::ConstInt {
                dst: want_t,
                ty: Type::U64,
                value: want as i128,
            });
            let mut result = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: result,
                op: BinOp::Eq,
                ty: Type::U64,
                lhs: tag_t,
                rhs: want_t,
            });
            for (i, subpat) in payload.iter().enumerate() {
                let payload_t = b.fresh(subpat.ty.clone());
                b.emit(Inst::EnumPayload {
                    dst: payload_t,
                    src: value,
                    index: i,
                });
                let sub_ok = lower_pattern_test(subpat, payload_t, bindings, b)?;
                let merged = b.fresh(Type::Bool);
                b.emit(Inst::BoolAnd {
                    dst: merged,
                    lhs: result,
                    rhs: sub_ok,
                });
                result = merged;
            }
            Ok(result)
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            let result = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool {
                dst: result,
                value: true,
            });
            let mut result = result;
            for (i, subpat) in items.iter().enumerate() {
                let elem_t = b.fresh(subpat.ty.clone());
                b.emit(Inst::Project {
                    dst: elem_t,
                    base: value,
                    index: i,
                });
                let sub_ok = lower_pattern_test(subpat, elem_t, bindings, b)?;
                let merged = b.fresh(Type::Bool);
                b.emit(Inst::BoolAnd {
                    dst: merged,
                    lhs: result,
                    rhs: sub_ok,
                });
                result = merged;
            }
            Ok(result)
        }
        TypedPatternKind::Or(_) => Err(LowerError::unimplemented("an `|` (or) pattern is")),
    }
}

// --- places (assignment targets) ---------------------------------------

/// Materialize `place` into a temp that a subsequent `SetField` /
/// `IndexSet` / `mut` write-back can mutate. A bare local is its own
/// frame slot (`needs_writeback = false`); a field/index chain is
/// projected out as a copy that the caller must write back through
/// `lower_place_write` after mutating (`needs_writeback = true`).
/// plans/M9.md item MM: dumb RMW — no address caching, no CSE.
fn materialize_place_mut(
    place: &TypedExpr,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(Temp, bool), LowerError> {
    match &place.kind {
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name).ok_or_else(|| {
                LowerError::internal(format!("unbound local `{name}` in place position"))
            })?;
            Ok((t, false))
        }
        TypedExprKind::Field(..) | TypedExprKind::Index(..) => {
            let t = lower_expr(place, b, env)?;
            Ok((t, true))
        }
        _ => Err(LowerError::internal(
            "expression is not an assignable place",
        )),
    }
}

/// Writes `value` into `target`'s place — bare local, or an arbitrary
/// depth chain of field/index projections (plans/M9.md item MM). Nested
/// places project out, mutate, and write the modified aggregate back;
/// a bare-local base is mutated in place via `SetField`/`IndexSet`.
fn lower_place_write(
    target: &TypedExpr,
    value: Temp,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(), LowerError> {
    match &target.kind {
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name).ok_or_else(|| {
                LowerError::internal(format!("unbound local `{name}` in place position"))
            })?;
            b.emit(Inst::Copy { dst: t, src: value });
            Ok(())
        }
        TypedExprKind::Field(base, fname) => {
            // plans/M10.md item A2c / decision 587: assign through a placed
            // static's named field — MmioWrite at the layout offset.
            if let TypedExprKind::Static(sname) = &base.kind {
                let layout_name = match bodies::unwrap_own(base.ty.clone()) {
                    Type::Named(n, _) => n,
                    other => {
                        return Err(LowerError::internal(format!(
                            "placed static `{sname}` has non-named type {other:?}"
                        )));
                    }
                };
                let offset = runtime_layout_field_offset(&layout_name, fname, b.prog())?;
                let base_temp = lower_expr(base, b, env)?;
                b.emit(Inst::MmioWrite {
                    base: base_temp,
                    offset,
                    ty: target.ty.clone(),
                    value,
                });
                return Ok(());
            }
            // plans/M7.md item G, decision 17: assigning an `InterruptCell`
            // field of `self` must STLR the live driver-state word. Only
            // bare `self.<cell>` has a known live offset; nested chains
            // refuse rather than write a frame copy and skip the STLR.
            if bodies::is_interrupt_cell_type(&target.ty) {
                let TypedExprKind::Local(base_name) = &base.kind else {
                    return Err(LowerError::unimplemented(
                        "assigning an `InterruptCell` through a nested field/index chain is",
                    ));
                };
                if base_name != "self" {
                    return Err(LowerError::unimplemented(
                        "assigning an `InterruptCell` on a non-`self` place is",
                    ));
                }
                let base_temp = env_lookup(env, base_name)
                    .ok_or_else(|| LowerError::internal(format!("unbound local `{base_name}`")))?;
                let base_ty = bodies::unwrap_own(base.ty.clone());
                let idx = field_index(b.prog(), &base_ty, fname)?;
                let field_off = interrupt_cell_field_off(b, &base_ty, idx)?;
                b.emit(Inst::InterruptCellStoreRelease {
                    field_off,
                    width: 4,
                    value,
                });
                b.emit(Inst::SetField {
                    base: base_temp,
                    index: idx,
                    value,
                });
                return Ok(());
            }
            let (base_temp, needs_writeback) = materialize_place_mut(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let idx = field_index(b.prog(), &base_ty, fname)?;
            b.emit(Inst::SetField {
                base: base_temp,
                index: idx,
                value,
            });
            if needs_writeback {
                lower_place_write(base, base_temp, b, env)?;
            }
            Ok(())
        }
        TypedExprKind::Index(base, idx_expr) => {
            let (base_temp, needs_writeback) = materialize_place_mut(base, b, env)?;
            let idx_temp = lower_expr(idx_expr, b, env)?;
            let len = eval_array_len(&base.ty)?;
            b.emit(Inst::IndexSet {
                base: base_temp,
                index: idx_temp,
                value,
                len,
            });
            if needs_writeback {
                lower_place_write(base, base_temp, b, env)?;
            }
            Ok(())
        }
        _ => Err(LowerError::internal(
            "expression is not an assignable place",
        )),
    }
}

// --- calls ----------------------------------------------------------------

/// Lowers one `mut`-mode call-site operand to the place temp that will
/// also appear in `Inst::Call::write_backs`. Nested field/index places
/// project out as a scratch temp; the caller must write that temp back
/// through `lower_place_write` after the call (plans/M9.md item MM). A
/// bare local is mutated in place — no write-back place.
fn lower_mut_arg_place<'a>(
    expr: &'a TypedExpr,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(Temp, Option<&'a TypedExpr>), LowerError> {
    match &expr.kind {
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name).ok_or_else(|| {
                LowerError::internal(format!("unbound local `{name}` as `mut` argument"))
            })?;
            Ok((t, None))
        }
        TypedExprKind::Field(..) | TypedExprKind::Index(..) => {
            let t = lower_expr(expr, b, env)?;
            Ok((t, Some(expr)))
        }
        _ => Err(LowerError::internal(
            "expression is not an assignable `mut` place",
        )),
    }
}

/// Evaluates a call's own argument slots against the callee's declared
/// parameters exactly like `interp::bind_params`: a supplied slot
/// lowers in the *caller's* environment; an elided (defaulted) slot's
/// own stored default (`TypedParam::default`) lowers in a small,
/// progressively-growing "callee-shaped" environment seeded with `self`
/// (if any) and every earlier parameter's own just-lowered temp — a
/// default may reference either, `sema::bodies::check_params_with_defaults`'s
/// own typing order, mirrored here one stage later. A `mut` parameter's
/// supplied operand lowers through `lower_mut_arg_place` so the temp
/// passed is the place itself (required for epilogue write-back). Nested
/// `mut` places return a post-call write-back obligation in
/// `nested_mut_writebacks` (plans/M9.md item MM).
fn bind_args<'a>(
    f: &TypedFn,
    args: &'a [Option<TypedExpr>],
    self_temp: Option<Temp>,
    b: &mut FnBuilder,
    caller_env: &mut LEnv,
    nested_mut_writebacks: &mut Vec<(&'a TypedExpr, Temp)>,
) -> Result<Vec<Temp>, LowerError> {
    let mut callee_env: LEnv = vec![BTreeMap::new()];
    if let Some(st) = self_temp {
        env_insert(&mut callee_env, "self".to_string(), st);
    }
    let mut out = Vec::with_capacity(args.len());
    for (param, slot) in f.params.iter().zip(args.iter()) {
        let t = match slot {
            Some(e) if param.mode == AccessMode::Mut => {
                let (t, wb) = lower_mut_arg_place(e, b, caller_env)?;
                if let Some(place) = wb {
                    nested_mut_writebacks.push((place, t));
                }
                t
            }
            Some(e) => lower_expr(e, b, caller_env)?,
            None if param.mode == AccessMode::Mut => {
                return Err(LowerError::unimplemented(
                    "writing back a `mut` parameter through a defaulted argument is",
                ));
            }
            None => {
                let default = param
                    .default
                    .as_ref()
                    .expect("producer guarantees a default when a call slot is None");
                lower_expr(default, b, &mut callee_env)?
            }
        };
        env_insert(&mut callee_env, param.name.clone(), t);
        out.push(t);
    }
    Ok(out)
}

/// Builds `Inst::Call::write_backs`: a `Mut` receiver at args index 0
/// (when present) plus every non-receiver `mut` parameter, args-indexed
/// with the receiver (if any) occupying slot 0.
fn call_write_backs(
    f: &TypedFn,
    receiver_temp: Option<Temp>,
    arg_temps: &[Temp],
) -> Vec<(usize, Temp)> {
    let mut write_backs = Vec::new();
    let arg0_is_receiver = receiver_temp.is_some();
    if let Some(st) = receiver_temp {
        if matches!(f.receiver.as_ref().map(|(m, _)| *m), Some(AccessMode::Mut)) {
            write_backs.push((0, st));
        }
    }
    for (i, param) in f.params.iter().enumerate() {
        if param.mode == AccessMode::Mut {
            let args_idx = if arg0_is_receiver { i + 1 } else { i };
            write_backs.push((args_idx, arg_temps[i]));
        }
    }
    write_backs
}

/// Dispatches one `Call` node (`interp::eval_call`'s own callee-key
/// dispatch, one stage later): resolves the target, evaluates the
/// receiver per its own declared mode, and emits the call.
fn lower_call(
    callee: &CalleeKey,
    receiver: &Option<Box<TypedExpr>>,
    args: &[Option<TypedExpr>],
    result_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    // plans/M9.md item C2: core-scalar `.format()` — no TypedFn exists.
    if let CalleeKey::Method(_, m) = callee {
        if m == "format" {
            if let Some(recv) = receiver {
                if let Some(_k) = crate::sema::types::scalar_format_bound(&recv.ty) {
                    let src = lower_expr(recv, b, env)?;
                    let Type::String(n_expr) = result_ty else {
                        return Err(LowerError::internal(
                            "scalar `.format()` result is not `String[..N]`".to_string(),
                        ));
                    };
                    let capacity = eval_len_expr(n_expr)?;
                    let dst = b.fresh(result_ty.clone());
                    b.emit(Inst::FormatScalar {
                        dst,
                        src,
                        src_ty: recv.ty.clone(),
                        capacity,
                    });
                    return Ok(dst);
                }
            }
        }
    }
    let member_is_init =
        matches!(callee, CalleeKey::Method(_, m) | CalleeKey::MethodInstance(_, m) if m == "init");
    let f = resolve_fn(b.prog(), callee).ok_or_else(|| missing_callee(b.prog(), callee))?;
    let key = callee.spelling();

    if member_is_init {
        return lower_init_call(f, &key, args, result_ty, b, env);
    }

    let mode = f.receiver.as_ref().map(|(m, _)| *m);
    match (receiver, mode) {
        (Some(recv_expr), Some(AccessMode::Mut)) => {
            let (self_temp, recv_wb) = lower_mut_arg_place(recv_expr, b, env)?;
            let mut nested_mut_writebacks = Vec::new();
            if let Some(place) = recv_wb {
                nested_mut_writebacks.push((place, self_temp));
            }
            let arg_temps =
                bind_args(f, args, Some(self_temp), b, env, &mut nested_mut_writebacks)?;
            let write_backs = call_write_backs(f, Some(self_temp), &arg_temps);
            let mut call_args = vec![self_temp];
            call_args.extend(arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs,
                key,
                args: call_args,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
        (Some(recv_expr), Some(AccessMode::Read | AccessMode::Take)) => {
            let recv_temp = lower_expr(recv_expr, b, env)?;
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps =
                bind_args(f, args, Some(recv_temp), b, env, &mut nested_mut_writebacks)?;
            let write_backs = call_write_backs(f, Some(recv_temp), &arg_temps);
            let mut call_args = vec![recv_temp];
            call_args.extend(arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs,
                key,
                args: call_args,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
        _ => {
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps = bind_args(f, args, None, b, env, &mut nested_mut_writebacks)?;
            let write_backs = call_write_backs(f, None, &arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs,
                key,
                args: arg_temps,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
    }
}

/// `init`'s own call-site translation — `interp::run_init`, one stage
/// later: allocates a fresh, uninitialized `self` (flow's definite-init
/// pass already proved every field is assigned before any real exit, so
/// no placeholder value is needed, just a slot), runs the body with it
/// as the receiver, then reinterprets the body's own result exactly like
/// `run_init` does — `Unit` ret: the call's result *is* the written-back
/// self (the body's own result value is discarded); `Result[Unit, E]`
/// ret: `Ok` wraps the written-back self, `Err` propagates unchanged.
fn lower_init_call(
    f: &TypedFn,
    key: &str,
    args: &[Option<TypedExpr>],
    result_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    let self_ty = f
        .receiver
        .as_ref()
        .map(|(_, t)| t.clone())
        .ok_or_else(|| LowerError::internal("`init` has no receiver type"))?;
    let self_temp = b.fresh(self_ty);
    let mut nested_mut_writebacks = Vec::new();
    let arg_temps = bind_args(f, args, Some(self_temp), b, env, &mut nested_mut_writebacks)?;
    let write_backs = call_write_backs(f, Some(self_temp), &arg_temps);
    let mut call_args = vec![self_temp];
    call_args.extend(arg_temps);
    let body_dst = b.fresh(f.ret.clone());
    b.emit(Inst::Call {
        dst: body_dst,
        write_backs,
        key: key.to_string(),
        args: call_args,
    });
    for (place, t) in nested_mut_writebacks {
        lower_place_write(place, t, b, env)?;
    }
    match &f.ret {
        Type::Unit => Ok(self_temp),
        Type::Result(_, _) => {
            let Type::Result(_, err_ty) = result_ty else {
                return Err(LowerError::internal(
                    "`init`'s own call-result type is not `Result` even though its body's own return type is",
                ));
            };
            let tag_t = b.fresh(Type::U64);
            b.emit(Inst::EnumTag {
                dst: tag_t,
                src: body_dst,
            });
            let ok_const = b.fresh(Type::U64);
            b.emit(Inst::ConstInt {
                dst: ok_const,
                ty: Type::U64,
                value: value::RESULT_OK as i128,
            });
            let is_ok = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: is_ok,
                op: BinOp::Eq,
                ty: Type::U64,
                lhs: tag_t,
                rhs: ok_const,
            });
            let result = b.fresh(result_ty.clone());
            let else_fixup = b.emit(Inst::JumpIfFalse {
                cond: is_ok,
                target: usize::MAX,
            });
            b.emit(Inst::MakeEnum {
                dst: result,
                tag: value::RESULT_OK,
                payload: vec![self_temp],
            });
            let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let else_pos = b.here();
            b.patch_jump(else_fixup, else_pos);
            let err_payload = b.fresh((**err_ty).clone());
            b.emit(Inst::EnumPayload {
                dst: err_payload,
                src: body_dst,
                index: 0,
            });
            b.emit(Inst::MakeEnum {
                dst: result,
                tag: value::RESULT_ERR,
                payload: vec![err_payload],
            });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            Ok(result)
        }
        other => Err(LowerError::internal(format!(
            "`init` with a non-standard return type ({other:?}) — unreachable per sema"
        ))),
    }
}

// --- expressions ------------------------------------------------------------

/// Sync `?` (plans/M7.md item E1 / 02-language.md §7.4): on `Ok`, project
/// the payload; on `Err`, apply the target's `from` conversion when
/// needed (plans/M9.md item B), build this fn's own `Err`-wrapped return,
/// and early-return. Same shape as `flowwir_lower::lower_try_check`.
fn lower_try_sync(
    value_temp: Temp,
    value_ty: &Type,
    conv: &Option<CalleeKey>,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    let (ok_ty, err_ty) = match value_ty {
        Type::Result(o, e) => ((**o).clone(), (**e).clone()),
        _ => {
            return Err(LowerError::unimplemented(
                "`?` on a non-`Result` (e.g. `Option`) value in a synchronous body is",
            ));
        }
    };
    let tag_t = b.fresh(Type::U64);
    b.emit(Inst::EnumTag {
        dst: tag_t,
        src: value_temp,
    });
    let ok_const = b.fresh(Type::U64);
    b.emit(Inst::ConstInt {
        dst: ok_const,
        ty: Type::U64,
        value: value::RESULT_OK as i128,
    });
    let is_ok = b.fresh(Type::Bool);
    b.emit(Inst::Compare {
        dst: is_ok,
        op: BinOp::Eq,
        ty: Type::U64,
        lhs: tag_t,
        rhs: ok_const,
    });
    let err_fixup = b.emit(Inst::JumpIfFalse {
        cond: is_ok,
        target: usize::MAX,
    });
    let ok_payload = b.fresh(ok_ty);
    b.emit(Inst::EnumPayload {
        dst: ok_payload,
        src: value_temp,
        index: 0,
    });
    let after_fixup = b.emit(Inst::Jump { target: usize::MAX });
    let err_pos = b.here();
    b.patch_jump(err_fixup, err_pos);
    let err_payload = b.fresh(err_ty);
    b.emit(Inst::EnumPayload {
        dst: err_payload,
        src: value_temp,
        index: 0,
    });
    let Type::Result(_, ret_err) = &b.ret else {
        return Err(LowerError::internal(
            "`?` used inside a fn whose own declared return type is not `Result`".to_string(),
        ));
    };
    let target_ty = (**ret_err).clone();
    let converted = lower_from_conversion(err_payload, conv, target_ty, b)?;
    let ret_enum = b.fresh(b.ret.clone());
    b.emit(Inst::MakeEnum {
        dst: ret_enum,
        tag: value::RESULT_ERR,
        payload: vec![converted],
    });
    b.emit(Inst::Return {
        value: Some(ret_enum),
    });
    let after_pos = b.here();
    b.patch_jump(after_fixup, after_pos);
    Ok(ok_payload)
}

/// Apply `?`'s one-hop `from` conversion (02 §7.4 / plans/M9.md item B3).
/// Explicit and deriving-generated `from` are both Calls; there is no
/// structural wrap fallback.
fn lower_from_conversion(
    err_payload: Temp,
    conv: &Option<CalleeKey>,
    target_ty: Type,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    let Some(key) = conv else {
        return Ok(err_payload);
    };
    if resolve_fn(b.prog(), key).is_none() {
        return Err(LowerError::internal(format!(
            "`?` conversion `{}` has no TypedFn (deriving(From) must generate one)",
            key.spelling()
        )));
    }
    let dst = b.fresh(target_ty);
    b.emit(Inst::Call {
        dst,
        write_backs: Vec::new(),
        key: key.spelling(),
        args: vec![err_payload],
    });
    Ok(dst)
}

fn lower_expr(expr: &TypedExpr, b: &mut FnBuilder, env: &mut LEnv) -> Result<Temp, LowerError> {
    match &expr.kind {
        TypedExprKind::Int(text) => {
            let raw = value::parse_int_literal(text)
                .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
            match &expr.ty {
                Type::F32 => {
                    let dst = b.fresh(Type::F32);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F32,
                        bits: (raw as f32).to_bits() as u64,
                    });
                    Ok(dst)
                }
                Type::F64 => {
                    let dst = b.fresh(Type::F64);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F64,
                        bits: (raw as f64).to_bits(),
                    });
                    Ok(dst)
                }
                t => {
                    let dst = b.fresh(t.clone());
                    b.emit(Inst::ConstInt {
                        dst,
                        ty: t.clone(),
                        value: raw,
                    });
                    Ok(dst)
                }
            }
        }
        TypedExprKind::Float(text) => {
            let f: f64 = text
                .parse()
                .map_err(|_| LowerError::internal("invalid float literal text"))?;
            match &expr.ty {
                Type::F32 => {
                    let dst = b.fresh(Type::F32);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F32,
                        bits: (f as f32).to_bits() as u64,
                    });
                    Ok(dst)
                }
                _ => {
                    let dst = b.fresh(Type::F64);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F64,
                        bits: f.to_bits(),
                    });
                    Ok(dst)
                }
            }
        }
        TypedExprKind::Str(text) => {
            let bytes = value::decode_str(text);
            // plans/M9.md item C1: `String[..N]` is a fixed aggregate
            // (length word + N byte slots), not ConstText.
            if let Type::String(n_expr) = &expr.ty {
                return emit_string_aggregate(&bytes, n_expr, &expr.ty, b);
            }
            let data = b.intern(bytes);
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        TypedExprKind::BStr(text) => {
            let bytes = value::decode_bstr(text);
            let data = b.intern(bytes);
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        TypedExprKind::Char(text) => {
            let c = value::decode_char(text);
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstChar { dst, value: c });
            Ok(dst)
        }
        TypedExprKind::Bool(v) => {
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool { dst, value: *v });
            Ok(dst)
        }
        TypedExprKind::Unit => {
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        TypedExprKind::Local(name) => env_lookup(env, name)
            .ok_or_else(|| LowerError::internal(format!("unbound local `{name}`"))),
        TypedExprKind::Const(name) => {
            let v = crate::eval::interp::eval_const(b.prog(), name).map_err(|e| {
                LowerError::internal(format!(
                    "const `{name}` failed to evaluate during lowering \
                     (already proven to succeed by check_typed): {}",
                    e.message
                ))
            })?;
            emit_const_value(&v, &expr.ty, b)
        }
        // plans/M10.md item A2c / decision 587: a bare placed static is its
        // base address. Named-field access uses this temp as the MmioRead/
        // MmioWrite base.
        TypedExprKind::Static(name) => {
            let addr = placed_static_addr(b.prog(), name)?;
            let dst = b.fresh(Type::U64);
            b.emit(Inst::ConstInt {
                dst,
                ty: Type::U64,
                value: addr as i128,
            });
            Ok(dst)
        }
        TypedExprKind::FnRef(_) => Err(LowerError::unimplemented(
            "a bare fn/method value reference is",
        )),
        TypedExprKind::Field(base, name) => {
            // Named field of a placed static: MmioRead at the layout offset
            // (decision 587 — the Mmio[L] codegen shape, not its API).
            if let TypedExprKind::Static(sname) = &base.kind {
                let layout_name = match bodies::unwrap_own(base.ty.clone()) {
                    Type::Named(n, _) => n,
                    other => {
                        return Err(LowerError::internal(format!(
                            "placed static `{sname}` has non-named type {other:?}"
                        )));
                    }
                };
                let offset = runtime_layout_field_offset(&layout_name, name, b.prog())?;
                let base_temp = lower_expr(base, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::MmioRead {
                    dst,
                    base: base_temp,
                    offset,
                    ty: expr.ty.clone(),
                });
                return Ok(dst);
            }
            let base_temp = lower_expr(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            // plans/M9.md item E: Duration/Instant are scalar newtypes —
            // `.nanos` is the word itself.
            if let Type::Named(sname, _) = &base_ty {
                if matches!(sname.as_str(), "Duration" | "Instant") && name == "nanos" {
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::Copy {
                        dst,
                        src: base_temp,
                    });
                    return Ok(dst);
                }
            }
            let idx = field_index(b.prog(), &base_ty, name)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Project {
                dst,
                base: base_temp,
                index: idx,
            });
            Ok(dst)
        }
        TypedExprKind::Index(base, idx_expr) => {
            let base_temp = lower_expr(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            // plans/M9.md item C1: `String[..N][i]` with a literal index
            // projects slot `1+i` after a compile-time capacity check.
            // Dynamic indices (and occupied-length runtime checks beyond
            // the capacity) are refused by name — C1's guest pin uses
            // literal indices only.
            if let Type::String(n_expr) = &base_ty {
                let cap = eval_len_expr(n_expr)?;
                let i = match &idx_expr.kind {
                    TypedExprKind::Int(text) => {
                        let raw = value::parse_int_literal(text)
                            .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
                        usize::try_from(raw)
                            .map_err(|_| LowerError::internal("String index out of range"))?
                    }
                    _ => {
                        return Err(LowerError::unimplemented(
                            "indexing `String[..N]` with a non-literal index is",
                        ));
                    }
                };
                if i >= cap {
                    return Err(LowerError::internal(format!(
                        "String index {i} out of capacity {cap}"
                    )));
                }
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Project {
                    dst,
                    base: base_temp,
                    index: 1 + i,
                });
                return Ok(dst);
            }
            let idx_temp = lower_expr(idx_expr, b, env)?;
            let len = eval_array_len(&base.ty)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::IndexGet {
                dst,
                base: base_temp,
                index: idx_temp,
                len,
            });
            Ok(dst)
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => lower_call(callee, receiver, args, &expr.ty, b, env),
        TypedExprKind::CallValue(..) => Err(LowerError::unimplemented(
            "calling a closure/fn value indirectly is",
        )),
        TypedExprKind::ToScalar(inner) => {
            let src = lower_expr(inner, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Convert {
                dst,
                ty: expr.ty.clone(),
                src,
                abort: mwir::convert_abort_message(&expr.ty),
            });
            Ok(dst)
        }
        TypedExprKind::Neg(inner) => {
            // Mirrors `interp::eval_expr`'s own `Neg` arm exactly,
            // including its own doc comment's reasoning: a negated
            // integer *literal* (`i8::MIN`, ...) must decode and negate
            // in `i128` directly, never truncate-then-negate (which
            // double-wraps exactly the MIN literals).
            if let TypedExprKind::Int(text) = &inner.kind {
                let raw = value::parse_int_literal(text)
                    .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::ConstInt {
                    dst,
                    ty: expr.ty.clone(),
                    value: -raw,
                });
                Ok(dst)
            } else {
                let src = lower_expr(inner, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Neg {
                    dst,
                    ty: expr.ty.clone(),
                    src,
                    abort: mwir::neg_abort_message(),
                });
                Ok(dst)
            }
        }
        TypedExprKind::BitNot(inner) => {
            let src = lower_expr(inner, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::BitNot {
                dst,
                ty: expr.ty.clone(),
                src,
            });
            Ok(dst)
        }
        // Decision 2: "a take is a copy in mwir" — an ordinary read of
        // `inner` is already observationally identical to a real move
        // (`interp.rs`'s own `Take` arm: literally `eval_expr(inner, ...)`,
        // no wrapping at all). Whichever context consumes this temp
        // (a `Let`'s own destination temp, a call argument slot, an
        // aggregate element) already gets its own distinct copy at *that*
        // point, so nothing extra happens here.
        TypedExprKind::Take(inner) => lower_expr(inner, b, env),
        TypedExprKind::Try(inner, conv) => {
            // plans/M7.md item E1 / plans/M9.md item B: sync `?`
            // (02-language.md §7.4). A driver's fallible `init` is a plain
            // `fn` and must be able to `?`-propagate `BootError`. Active
            // defers at the early-exit site are not run here — a driver's
            // `init` that uses both `defer` and `?` fails closed by name
            // rather than silently skipping cleanup (none of E1's goldens
            // combine the two).
            let v = lower_expr(inner, b, env)?;
            lower_try_sync(v, &inner.ty, conv, b)
        }
        TypedExprKind::Binary(op, l, r) => lower_binary(*op, l, r, expr, b, env),
        TypedExprKind::OpCall(key, l, r) => {
            // A user (`Named`) type's desugared operator method
            // (`typed::TypedExprKind::OpCall`'s own doc comment): `self`
            // (the left operand), then the right-hand operand — always
            // exactly one declared parameter, never a default (an
            // operator method's own signature is fixed by
            // 05-library.md §8), so this is `lower_call`'s own
            // read-receiver path with no argument-binding machinery
            // needed at all.
            let lv = lower_expr(l, b, env)?;
            let rv = lower_expr(r, b, env)?;
            let f = resolve_fn(b.prog(), key).ok_or_else(|| missing_callee(b.prog(), key))?;
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs: Vec::new(),
                key: key.spelling(),
                args: vec![lv, rv],
            });
            Ok(dst)
        }
        TypedExprKind::Is(inner, pattern) => {
            let v = lower_expr(inner, b, env)?;
            let mut bindings = BTreeMap::new();
            collect_pattern_bindings(pattern, &mut bindings, b);
            let test = lower_pattern_test(pattern, v, &bindings, b)?;
            // "its bindings flow into the success branch" (02-language.md
            // §7.2): inserted directly into the *current* scope, exactly
            // like `interp::eval_expr`'s own `Is` arm — no push/pop of
            // its own (whatever encloses this `Is` — an `if`'s own cond
            // — already scopes the branch that can legally read them;
            // sema is what actually keeps a read outside that branch from
            // ever type-checking, this mirrors interp.rs's own identical
            // shortcut).
            for (n, t) in bindings {
                env_insert(env, n, t);
            }
            Ok(test)
        }
        TypedExprKind::Not(inner) => {
            let v = lower_expr(inner, b, env)?;
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::Not { dst, src: v });
            Ok(dst)
        }
        TypedExprKind::And(l, r) => {
            let lv = lower_expr(l, b, env)?;
            let result = b.fresh(Type::Bool);
            let false_fixup = b.emit(Inst::JumpIfFalse {
                cond: lv,
                target: usize::MAX,
            });
            let rv = lower_expr(r, b, env)?;
            b.emit(Inst::Copy {
                dst: result,
                src: rv,
            });
            let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let false_pos = b.here();
            b.patch_jump(false_fixup, false_pos);
            b.emit(Inst::ConstBool {
                dst: result,
                value: false,
            });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            Ok(result)
        }
        TypedExprKind::Or(l, r) => {
            let lv = lower_expr(l, b, env)?;
            let result = b.fresh(Type::Bool);
            let eval_r_fixup = b.emit(Inst::JumpIfFalse {
                cond: lv,
                target: usize::MAX,
            });
            b.emit(Inst::ConstBool {
                dst: result,
                value: true,
            });
            let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let eval_r_pos = b.here();
            b.patch_jump(eval_r_fixup, eval_r_pos);
            let rv = lower_expr(r, b, env)?;
            b.emit(Inst::Copy {
                dst: result,
                src: rv,
            });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            Ok(result)
        }
        TypedExprKind::EnumConstruct {
            enum_name,
            variant,
            args,
        } => {
            let idx = variant_index(b.prog(), enum_name, variant)?;
            let mut arg_temps = Vec::with_capacity(args.len());
            for a in args {
                arg_temps.push(lower_expr(a, b, env)?);
            }
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeEnum {
                dst,
                tag: idx,
                payload: arg_temps,
            });
            Ok(dst)
        }
        TypedExprKind::Closure { .. } => Err(LowerError::unimplemented("a closure literal is")),
        TypedExprKind::Tuple(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for i in items {
                elems.push(lower_expr(i, b, env)?);
            }
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::List(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for i in items {
                elems.push(lower_expr(i, b, env)?);
            }
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::StructLiteral { name, fields } => {
            let Type::Named(sname, targs) = &expr.ty else {
                return Err(LowerError::internal("struct literal type is not `Named`"));
            };
            debug_assert_eq!(name, sname);
            // plans/M9.md item E: `Duration`/`Instant` are one-word
            // newtypes (private `nanos: u64`). Keep the pre-E ABI
            // (`is_aggregate` treats them as scalars) by lowering a
            // construction to a plain Copy of the nanos field.
            if matches!(sname.as_str(), "Duration" | "Instant") {
                if fields.len() != 1 {
                    return Err(LowerError::internal(format!(
                        "`{sname}` construction must supply exactly one field"
                    )));
                }
                let nanos = lower_expr(&fields[0].1, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Copy { dst, src: nanos });
                return Ok(dst);
            }
            let s = resolve_struct(b.prog(), sname, targs)
                .ok_or_else(|| missing_struct(b.prog(), sname))?;
            let mut slots: Vec<Option<Temp>> = vec![None; s.fields.len()];
            for (fname, fval) in fields {
                let idx = s
                    .fields
                    .iter()
                    .position(|f| f == fname)
                    .ok_or_else(|| LowerError::internal(format!("unknown field `{fname}`")))?;
                slots[idx] = Some(lower_expr(fval, b, env)?);
            }
            for (i, fname) in s.fields.iter().enumerate() {
                if slots[i].is_none() {
                    let default = s.field_defaults.get(fname).ok_or_else(|| {
                        LowerError::internal(format!(
                            "field `{fname}` has neither a supplied value nor a default"
                        ))
                    })?;
                    let mut fresh_env: LEnv = vec![BTreeMap::new()];
                    slots[i] = Some(lower_expr(default, b, &mut fresh_env)?);
                }
            }
            let elems: Vec<Temp> = slots
                .into_iter()
                .map(|s| s.expect("every slot filled above"))
                .collect();
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::Panic(msg) => {
            let text = assert_message_text(msg)?;
            b.emit(Inst::AssertFail {
                message: Some(format!("panic: {text}")),
            });
            // Unreachable past this point (`ty` is `never`) — the temp
            // is allocated only so this fn can return one uniformly; it
            // is never read.
            Ok(b.fresh(expr.ty.clone()))
        }
        // plans/M7.md item C (03-hardware.md §2): a typed MMIO register
        // access type-checks in full, and stops here — deliberately, and
        // not as a shortcut. **No `Mmio[L]` value can exist at runtime
        // today**: `eval::image_checks::check_capability_substitution`
        // rejects an `Mmio[L]` `init` parameter outright ("nothing mints a
        // `Mmio` yet"), and `layout::build_boot_init_calls` walks
        // `graph.actors` only — a `@driver`'s `init` is never called at
        // boot at all — and fails closed on every capability parameter
        // besides. Emitting a load/store against a base that is provably
        // the zero a state-fill left is the exact wrong answer plans/M7.md
        // item W exists to close, so this says so instead.
        // plans/M7.md item H1 (03-hardware.md §2/§9): the sealed
        // transport's own two operations. Both are pure *authority*
        // transitions on this target and lower to a `Copy` of decision
        // 11's one word — the device's own register-window base:
        //
        // - `claim` walks `Reset -> Acknowledged -> DriverClaimed`, which
        //   on a real virtio transport is three status-register writes and
        //   on machine v1 is nothing at all: 06-machine.md §3 deletes
        //   discovery and negotiation ("device topology is a *build
        //   output*"; "cold boot is a design property"), and the VMM has
        //   no status register file to write to. Emitting invented writes
        //   to a window no model reads would be worse than emitting none.
        // - `map_partition` hands out one of the driver's declared
        //   partitions. The partition's *offsets* live in the layout, so
        //   the value handed out is the claim's own base, unchanged; what
        //   makes the partitions disjoint is `check_mmio_claims`, at build
        //   time, over the same field set this operation is restricted to.
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } if crate::sema::bodies::is_device_transport_intrinsic(key) => {
            // plans/M7.md item E1: negotiate/start/configure are pure
            // plans/M7.md item E1: negotiate/start/configure are pure
            // authority transitions on this machine (decision 14: the
            // accepted feature set is a build-time fact; capacity is a
            // build constant). Each lowers to a Copy of the receiver's
            // word, or — for read_capacity — a ConstInt filled in at the
            // address pass from the image's declared capacity_sectors.
            // VirtQueue.configure yields the pool base (decision 11's one
            // word for the queue).
            match key.as_str() {
                "Device.take_irq" => {
                    // plans/M7.md item G, decision 12: the word is the vector
                    // bit index. Layout patches the reloc against this
                    // driver's `vector=` once the image graph is in hand.
                    let Some(driver) = b.owner_struct.clone() else {
                        return Err(LowerError::internal(
                            "`Device.take_irq` reached lowering outside a `@driver` member"
                                .to_string(),
                        ));
                    };
                    let _ = receiver; // authority already checked; the word does not need the base
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::LoadIrqVector { dst, driver });
                    Ok(dst)
                }
                "Device.read_capacity_sectors" => {
                    let capacity = b.blk_capacity_sectors().ok_or_else(|| {
                        LowerError::unimplemented(
                            "`read_capacity_sectors`: this image declares no \
                             `capacity_sectors=` on its `img.device` (plans/M7.md item E1: \
                             capacity is an image-declared build constant, not a register)",
                        )
                    })?;
                    let ok_payload = b.fresh(Type::U64);
                    b.emit(Inst::ConstInt {
                        dst: ok_payload,
                        ty: Type::U64,
                        value: capacity as i128,
                    });
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::MakeEnum {
                        dst,
                        tag: value::RESULT_OK,
                        payload: vec![ok_payload],
                    });
                    Ok(dst)
                }
                "Device.negotiate" | "VirtQueue.configure" => {
                    // Both return Result[T, BootError]. The Ok payload is
                    // the authority word: negotiate copies the claimed
                    // device's base; configure copies the pool's base.
                    let src = match (key.as_str(), receiver, args.as_slice()) {
                        ("Device.negotiate", Some(state), _) => lower_expr(state, b, env)?,
                        ("VirtQueue.configure", _, args) => {
                            let (_, pool) =
                                args.iter().find(|(l, _)| l == "pool").ok_or_else(|| {
                                    LowerError::internal(
                                        "`VirtQueue.configure` reached lowering without `pool=`"
                                            .to_string(),
                                    )
                                })?;
                            lower_expr(pool, b, env)?
                        }
                        _ => {
                            return Err(LowerError::internal(format!(
                                "sealed-transport intrinsic `{key}` reached lowering without \
                                 its operand"
                            )));
                        }
                    };
                    let Type::Result(ok_ty, _) = &expr.ty else {
                        return Err(LowerError::internal(format!(
                            "`{key}`'s typed result is not a `Result`"
                        )));
                    };
                    let payload = b.fresh((**ok_ty).clone());
                    b.emit(Inst::Copy { dst: payload, src });
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::MakeEnum {
                        dst,
                        tag: value::RESULT_OK,
                        payload: vec![payload],
                    });
                    Ok(dst)
                }
                "Device.start" | "Device.claim" | "Device.map_partition" => {
                    let src = match (key.as_str(), receiver, args.first()) {
                        ("Device.claim", _, Some((_, cap))) => lower_expr(cap, b, env)?,
                        ("Device.map_partition" | "Device.start", Some(state), _) => {
                            lower_expr(state, b, env)?
                        }
                        _ => {
                            return Err(LowerError::internal(format!(
                                "sealed-transport intrinsic `{key}` reached lowering without \
                                 its operand"
                            )));
                        }
                    };
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::Copy { dst, src });
                    Ok(dst)
                }
                "Device.reset" => {
                    let device = match receiver {
                        Some(state) => lower_expr(state, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`Device.reset` reached lowering without a RunningDevice receiver"
                                    .to_string(),
                            ));
                        }
                    };
                    let queue = match args.first() {
                        Some((_, q)) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`Device.reset` reached lowering without a queue argument"
                                    .to_string(),
                            ));
                        }
                    };
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::DeviceReset { dst, device, queue });
                    Ok(dst)
                }
                other => Err(LowerError::internal(format!(
                    "unknown sealed-transport intrinsic `{other}`"
                ))),
            }
        }
        // plans/M7.md item G: bind/unmask are build-time facts for the
        // vector table (collected from the typed tree by
        // `eval::image_checks::check_vector_bindings`). At runtime they
        // are no-ops — the table is already wired, and this machine has
        // no per-vector mask bit in the pending word (06 §4; the
        // InterruptCell level signal is the ISR/ordinary channel).
        TypedExprKind::Intrinsic { key, .. } if crate::sema::bodies::is_irq_cap_intrinsic(key) => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        // plans/M7.md item G, decision 17: `InterruptCell[T]` ops. Every
        // method addresses the live cell at `self_ptr + field_off`.
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } if crate::sema::bodies::is_interrupt_cell_intrinsic(key) => {
            lower_interrupt_cell_intrinsic(key, receiver.as_deref(), args, &expr.ty, b, env)
        }
        // plans/M7.md item G: `wake(Driver.task)` — sticky wake-pending bit.
        TypedExprKind::Intrinsic { key, args, .. }
            if crate::sema::bodies::is_wake_intrinsic(key) =>
        {
            let Some((_, task)) = args.iter().find(|(l, _)| l == "task") else {
                return Err(LowerError::internal(
                    "`wake` with no task argument".to_string(),
                ));
            };
            let TypedExprKind::FnRef(callee) = &task.kind else {
                return Err(LowerError::internal(
                    "`wake` task is not a FnRef".to_string(),
                ));
            };
            let driver = match callee {
                crate::sema::typed::CalleeKey::Method(s, _) => s.clone(),
                crate::sema::typed::CalleeKey::MethodInstance(ikey, _) => ikey
                    .strip_prefix("struct:")
                    .unwrap_or(ikey.as_str())
                    .split('[')
                    .next()
                    .unwrap_or(ikey.as_str())
                    .to_string(),
                _ => {
                    return Err(LowerError::internal(
                        "`wake` task is not a driver method".to_string(),
                    ));
                }
            };
            b.emit(Inst::Wake { driver });
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        // plans/M7.md item H1: a typed MMIO access, emitted at last. The
        // base is the `Mmio[L]` receiver's own word; the offset and the
        // width both come from the declaration, looked up in the same
        // `check_layouts` table the checker used.
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
        } if crate::sema::bodies::is_mmio_access_intrinsic(key) => {
            let Some(mmio) = receiver else {
                return Err(LowerError::internal(
                    "an MMIO access with no receiver".to_string(),
                ));
            };
            let (layout_name, register) = mmio_access_names(&mmio.ty, args)?;
            let offset = mmio_register_offset(&layout_name, &register, b.prog())?;
            let Some(ty) = type_arg.clone() else {
                return Err(LowerError::internal(
                    "an MMIO access with no register type".to_string(),
                ));
            };
            let base = lower_expr(mmio, b, env)?;
            if key == "Mmio.read" {
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::MmioRead {
                    dst,
                    base,
                    offset,
                    ty,
                });
                Ok(dst)
            } else {
                let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                    return Err(LowerError::internal(
                        "an `Mmio.write` with no value argument".to_string(),
                    ));
                };
                let value = lower_expr(v, b, env)?;
                b.emit(Inst::MmioWrite {
                    base,
                    offset,
                    ty,
                    value,
                });
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::ConstUnit { dst });
                Ok(dst)
            }
        }
        // plans/M7.md item H2a, 03-hardware.md §8: `reported.checked_le(bound)`.
        // A real compare and a real branch — not a cast, not a no-op. The
        // `Untrusted[T]` receiver is a transparent newtype over `T` at the
        // ABI (`mwir::size_of`), so lowering it is just lowering its bits;
        // success builds `Ok(payload)`, failure builds `Err(unit)`.
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
        } if crate::sema::bodies::is_untrusted_narrowing_intrinsic(key) => {
            lower_untrusted_checked_le(expr, receiver, type_arg, args, b, env)
        }
        // plans/M7.md item E2/E3, 03-hardware.md §4/§5 / decision 15/16:
        // reserve/prepare package; publish emits the sealed write-order
        // node (real DRAM stores wait for E4's pool-backed addresses);
        // reject mints a resolved receipt without touching the ring.
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
        } if crate::sema::bodies::is_queue_op_intrinsic(key) => {
            match key.as_str() {
                "VirtQueue.prepare_block" => {
                    // plans/M7.md item E4 / decision 20: package header/
                    // status into the control pool and record the payload
                    // address. Payload length is the `@layout(dma)` size.
                    let permit = args.iter().find(|(l, _)| l == "permit").ok_or_else(|| {
                        LowerError::internal(
                            "`prepare_block` reached lowering without `permit=`".to_string(),
                        )
                    })?;
                    let header = args.iter().find(|(l, _)| l == "header").ok_or_else(|| {
                        LowerError::internal(
                            "`prepare_block` reached lowering without `header=`".to_string(),
                        )
                    })?;
                    let payload = args.iter().find(|(l, _)| l == "payload").ok_or_else(|| {
                        LowerError::internal(
                            "`prepare_block` reached lowering without `payload=`".to_string(),
                        )
                    })?;
                    let status = args.iter().find(|(l, _)| l == "status").ok_or_else(|| {
                        LowerError::internal(
                            "`prepare_block` reached lowering without `status=`".to_string(),
                        )
                    })?;
                    let device_writes_arg = args
                        .iter()
                        .find(|(l, _)| l == "device_writes_payload")
                        .ok_or_else(|| {
                            LowerError::internal(
                                "`prepare_block` reached lowering without `device_writes_payload=`"
                                    .to_string(),
                            )
                        })?;
                    let device_writes = match &device_writes_arg.1.kind {
                        TypedExprKind::Bool(v) => *v,
                        _ => {
                            return Err(LowerError::unimplemented(
                                "`prepare_block`'s `device_writes_payload=` as a non-literal bool \
                                 (revision 0.1 requires a literal `true`/`false`)",
                            ));
                        }
                    };
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`prepare_block` reached lowering without a queue receiver"
                                    .to_string(),
                            ));
                        }
                    };
                    let permit_t = lower_expr(&permit.1, b, env)?;
                    let header_t = lower_expr(&header.1, b, env)?;
                    let payload_t = lower_expr(&payload.1, b, env)?;
                    let status_t = lower_expr(&status.1, b, env)?;
                    let payload_len =
                        layout_dma_size(&payload.1.ty, b.prog()).ok_or_else(|| {
                            LowerError::internal(
                            "`prepare_block`'s payload type has no `@layout(dma)` size in this \
                             program"
                                .to_string(),
                        )
                        })?;
                    if payload_len == 0 || payload_len % 512 != 0 {
                        return Err(LowerError::unimplemented(&format!(
                            "`prepare_block` with payload layout size {payload_len}: the virtio-blk \
                             model requires a positive multiple of 512 (SECTOR_SIZE)"
                        )));
                    }
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::QueuePrepare {
                        dst,
                        queue,
                        permit: permit_t,
                        header: header_t,
                        payload: payload_t,
                        status: status_t,
                        device_writes,
                        payload_len: payload_len as u32,
                    });
                    Ok(dst)
                }
                "VirtQueue.reserve_proven" => {
                    // plans/M7.md item E4 / decision 20: the permit word is
                    // the descriptor-table head (single-flight: always 0).
                    // The `descriptors=` argument is proof-only.
                    let _ = args
                        .iter()
                        .find(|(l, _)| l == "descriptors")
                        .ok_or_else(|| {
                            LowerError::internal(
                                "`reserve_proven` reached lowering without `descriptors=`"
                                    .to_string(),
                            )
                        })?;
                    let _ = receiver;
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::ConstInt {
                        dst,
                        ty: Type::U64,
                        value: 0,
                    });
                    Ok(dst)
                }
                "VirtQueue.publish" => {
                    let op = args.iter().find(|(l, _)| l == "operation").ok_or_else(|| {
                        LowerError::internal(
                            "`publish` reached lowering without `operation=`".to_string(),
                        )
                    })?;
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`publish` reached lowering without a queue receiver".to_string(),
                            ));
                        }
                    };
                    let operation = lower_expr(&op.1, b, env)?;
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::QueuePublish {
                        dst,
                        queue,
                        operation,
                        steps: crate::virtqueue::PUBLISH_WRITE_ORDER,
                    });
                    Ok(dst)
                }
                "VirtQueue.reject" => {
                    // Consume payload + error; mint a Receipt word (opaque).
                    // Revision 0.1: reject still mints 0 — `await` of a
                    // rejected receipt is fail-closed until reject writes a
                    // resolved IoCompletion stash (flagship does not reject).
                    for (_, a) in args {
                        let _ = lower_expr(a, b, env)?;
                    }
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::ConstInt {
                        dst,
                        ty: Type::U64,
                        value: 0,
                    });
                    let _ = receiver;
                    Ok(dst)
                }
                "VirtQueue.drain" => {
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`drain` reached lowering without a queue receiver".to_string(),
                            ));
                        }
                    };
                    // `check_virtqueue_drain` folds `max=` into `type_arg`'s Bound.
                    let max_val = match type_arg {
                        Some(Type::Named(_, targs)) => match targs.first() {
                            Some(crate::sema::types::TypeArg::Bound(
                                crate::syntax::ast::Expr::Int(_, text),
                            )) => text
                                .parse::<u16>()
                                .map_err(|_| LowerError::internal(format!("drain max `{text}`")))?,
                            _ => {
                                return Err(LowerError::internal(
                                    "`drain` type_arg Bound is not an integer literal".to_string(),
                                ));
                            }
                        },
                        _ => {
                            return Err(LowerError::internal(
                                "`drain` reached lowering without a folded max Bound".to_string(),
                            ));
                        }
                    };
                    let _ = args;
                    b.emit(Inst::QueueDrain {
                        queue,
                        max: max_val,
                    });
                    let dst = b.fresh(Type::Unit);
                    b.emit(Inst::ConstUnit { dst });
                    Ok(dst)
                }
                "VirtQueue.suppress_interrupts" => {
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`suppress_interrupts` reached lowering without a queue receiver"
                                    .to_string(),
                            ));
                        }
                    };
                    b.emit(Inst::QueueSuppressInterrupts { queue });
                    let dst = b.fresh(Type::Unit);
                    b.emit(Inst::ConstUnit { dst });
                    Ok(dst)
                }
                "VirtQueue.claim" => {
                    let receipt_arg =
                        args.iter().find(|(l, _)| l == "receipt").ok_or_else(|| {
                            LowerError::internal(
                                "`claim` reached lowering without `receipt=`".to_string(),
                            )
                        })?;
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`claim` reached lowering without a queue receiver".to_string(),
                            ));
                        }
                    };
                    let receipt = lower_expr(&receipt_arg.1, b, env)?;
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::QueueClaim {
                        dst,
                        queue,
                        receipt,
                    });
                    Ok(dst)
                }
                "VirtQueue.recover" => {
                    let receipt_arg =
                        args.iter().find(|(l, _)| l == "receipt").ok_or_else(|| {
                            LowerError::internal(
                                "`recover` reached lowering without `receipt=`".to_string(),
                            )
                        })?;
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`recover` reached lowering without a queue receiver".to_string(),
                            ));
                        }
                    };
                    let receipt = lower_expr(&receipt_arg.1, b, env)?;
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::QueueRecover {
                        dst,
                        queue,
                        receipt,
                    });
                    Ok(dst)
                }
                "VirtQueue.reclaim" => {
                    // `pool=`/`payload=` are *declarations* read by sema —
                    // a bound pool name and a `@layout(dma)` type name,
                    // neither of which has a value form. The handle the
                    // gate hands back is the quarantined slot's own
                    // payload word, so nothing here is lowered.
                    let queue = match receiver {
                        Some(q) => lower_expr(q, b, env)?,
                        None => {
                            return Err(LowerError::internal(
                                "`reclaim` reached lowering without a queue receiver".to_string(),
                            ));
                        }
                    };
                    let _ = args;
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::QueueReclaim { dst, queue });
                    Ok(dst)
                }
                other => Err(LowerError::internal(format!(
                    "unknown queue-op intrinsic `{other}`"
                ))),
            }
        }
        TypedExprKind::Intrinsic { key, .. }
            if let Some(owner) = crate::sema::bodies::is_queue_op_deferred(key) =>
        {
            Err(LowerError::unimplemented(&format!("`{key}` ({owner}) is")))
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } if key == "Array.map_take" || key == "Array.try_map_take" => {
            lower_array_map_take(key, receiver.as_deref(), args, &expr.ty, b, env)
        }
        TypedExprKind::Intrinsic { .. } => Err(LowerError::unimplemented(
            "an `@image` builder intrinsic (reachable only inside the one `@image` fn, which is never lowered) is",
        )),
        TypedExprKind::PoolName(_) => Err(LowerError::unimplemented(
            "a bare pool name (the `@image` builder surface) is",
        )),
        // Plans/M6.md item A: `await`/`send`/a group-child reference are
        // all suspension-bearing constructs — FlowWir (item B) is the
        // typed, suspension-explicit IR between the typed tree and mwir
        // that actually lowers them; this pass (M5's straight-line sync
        // lowering) fails closed, named, rather than mis-lowering one as
        // ordinary sync code.
        TypedExprKind::Await(_) => Err(LowerError::unimplemented(
            "an `await` expression (FlowWir, plans/M6.md item B) is",
        )),
        TypedExprKind::Send(_) => Err(LowerError::unimplemented(
            "a `send` expression (FlowWir, plans/M6.md item B) is",
        )),
        TypedExprKind::GroupChild(_) => Err(LowerError::unimplemented(
            "a group-child reference (FlowWir, plans/M6.md item B) is",
        )),
    }
}

fn lower_binary(
    op: BinOp,
    l: &TypedExpr,
    r: &TypedExpr,
    expr: &TypedExpr,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    // plans/M9.md item C2: `String[..N] + String[..M] -> String[..N+M]`.
    if op == BinOp::Add {
        if let (Type::String(ln), Type::String(rn), Type::String(_)) = (&l.ty, &r.ty, &expr.ty) {
            let lhs_cap = eval_len_expr(ln)?;
            let rhs_cap = eval_len_expr(rn)?;
            let lv = lower_expr(l, b, env)?;
            let rv = lower_expr(r, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::StringConcat {
                dst,
                lhs: lv,
                rhs: rv,
                lhs_cap,
                rhs_cap,
            });
            return Ok(dst);
        }
    }
    let lv = lower_expr(l, b, env)?;
    let rv = lower_expr(r, b, env)?;
    let ty = l.ty.clone();
    let is_float = matches!(ty, Type::F32 | Type::F64);
    use BinOp::*;
    match op {
        Add | Sub | Mul => {
            let dst = b.fresh(expr.ty.clone());
            if is_float {
                b.emit(Inst::ArithWrapping {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                });
            } else {
                b.emit(Inst::ArithChecked {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                    abort: mwir::abort_message(op),
                });
            }
            Ok(dst)
        }
        AddW | SubW | MulW => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ArithWrapping {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
        Div | Rem => {
            let dst = b.fresh(expr.ty.clone());
            if is_float {
                b.emit(Inst::ArithWrapping {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                });
            } else {
                b.emit(Inst::DivRem {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                    abort_zero: mwir::div_zero_message(op),
                    abort_overflow: mwir::abort_message(op),
                });
            }
            Ok(dst)
        }
        Shl | Shr => {
            let dst = b.fresh(expr.ty.clone());
            let bits = int_bits(&ty)?;
            let lost = if op == Shl {
                Some(mwir::shift_lost_message())
            } else {
                None
            };
            b.emit(Inst::Shift {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
                bits,
                lost,
            });
            Ok(dst)
        }
        BitAnd | BitOr | BitXor => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Bitwise {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
        Lt | Le | Gt | Ge | Eq | Ne => {
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
    }
}

// --- const folding (module doc's own fail-closed enumeration) -----------

/// Materializes an already-evaluated comptime `Value` (a module `const`'s
/// own value — always comptime-fixed by the time `check_typed` succeeds)
/// into instructions. Scalars, `Str`/`Bytes`, and tuples/arrays *of*
/// those, fold exactly; a struct/enum/fn/closure/`@image`-decl value
/// fails closed (this module's own doc comment explains the real gap:
/// no struct/enum field-type table is threaded through this pass).
fn emit_const_value(v: &Value, ty: &Type, b: &mut FnBuilder) -> Result<Temp, LowerError> {
    match v {
        Value::U8(_)
        | Value::U16(_)
        | Value::U32(_)
        | Value::U64(_)
        | Value::Usize(_)
        | Value::I8(_)
        | Value::I16(_)
        | Value::I32(_)
        | Value::I64(_)
        | Value::Isize(_) => {
            let raw = value::as_i128(v).expect("checked above");
            let dst = b.fresh(ty.clone());
            b.emit(Inst::ConstInt {
                dst,
                ty: ty.clone(),
                value: raw,
            });
            Ok(dst)
        }
        Value::F32(x) => {
            let dst = b.fresh(Type::F32);
            b.emit(Inst::ConstFloat {
                dst,
                ty: Type::F32,
                bits: x.to_bits() as u64,
            });
            Ok(dst)
        }
        Value::F64(x) => {
            let dst = b.fresh(Type::F64);
            b.emit(Inst::ConstFloat {
                dst,
                ty: Type::F64,
                bits: x.to_bits(),
            });
            Ok(dst)
        }
        Value::Bool(x) => {
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool { dst, value: *x });
            Ok(dst)
        }
        Value::Char(c) => {
            let dst = b.fresh(Type::Char);
            b.emit(Inst::ConstChar { dst, value: *c });
            Ok(dst)
        }
        Value::Unit => {
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        Value::Str(bytes) => {
            if let Type::String(n_expr) = ty {
                return emit_string_aggregate(bytes, n_expr, ty, b);
            }
            let data = b.intern(bytes.clone());
            let dst = b.fresh(ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        Value::Bytes(bytes) => {
            let data = b.intern(bytes.clone());
            let dst = b.fresh(ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        Value::Tuple(items) | Value::Array(items) => {
            let elem_tys: Vec<Type> = match ty {
                Type::Tuple(ts) => ts.clone(),
                Type::Array(elem, _) => vec![(**elem).clone(); items.len()],
                other => {
                    return Err(LowerError::internal(format!(
                        "tuple/array const value with an unexpected type {other:?}"
                    )));
                }
            };
            let mut elems = Vec::with_capacity(items.len());
            for (item, ety) in items.iter().zip(elem_tys.iter()) {
                elems.push(emit_const_value(item, ety, b)?);
            }
            let dst = b.fresh(ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        Value::Struct(_) => Err(LowerError::unimplemented(
            "a struct-valued module `const` is",
        )),
        Value::Enum(_, _) => Err(LowerError::unimplemented(
            "an enum-valued module `const` is",
        )),
        Value::Fn(_) => Err(LowerError::unimplemented("a fn-valued module `const` is")),
        Value::Closure { .. } => Err(LowerError::unimplemented(
            "a closure-valued module `const` is",
        )),
        Value::ImageDecl(_) => Err(LowerError::unimplemented(
            "an `@image`-builder-valued module `const` is",
        )),
    }
}

// --- shared lookups (mirroring interp.rs's own, one stage later) --------

/// This module's own struct `name`, else the imported one bound to that
/// local name (plans/M9.md item EE — same union `eval::interp` already
/// applies; before EE an imported `Pair` abandoned with
/// `internal error: struct \`Pair\` not found`).
fn struct_by_name<'p>(prog: &'p TypedProgram, name: &str) -> Option<&'p TypedStruct> {
    prog.structs
        .get(name)
        .or_else(|| prog.imported.structs.get(name))
}

/// This module's own instantiation `key`, else the exporting module's.
fn instantiation_by_key<'p>(prog: &'p TypedProgram, key: &str) -> Option<&'p TypedInstantiation> {
    prog.instantiations
        .get(key)
        .or_else(|| prog.imported.instantiations.get(key))
}

fn resolve_fn<'p>(prog: &'p TypedProgram, key: &CalleeKey) -> Option<&'p TypedFn> {
    match key {
        CalleeKey::Fn(name) => prog.fns.get(name).or_else(|| prog.imported.fns.get(name)),
        CalleeKey::FnInstance(ikey) => match instantiation_by_key(prog, ikey) {
            Some(TypedInstantiation::Fn(f)) => Some(f),
            _ => None,
        },
        CalleeKey::Method(sname, member) => {
            if let Some(s) = struct_by_name(prog, sname) {
                return resolve_struct_member(s, member);
            }
            resolve_enum_member(enum_by_name(prog, sname)?, member)
        }
        CalleeKey::MethodInstance(ikey, member) => match instantiation_by_key(prog, ikey) {
            Some(TypedInstantiation::Struct(s)) => resolve_struct_member(s, member),
            _ => None,
        },
    }
}

fn resolve_struct_member<'p>(s: &'p TypedStruct, member: &str) -> Option<&'p TypedFn> {
    if member == "init" {
        s.init.as_ref()
    } else {
        s.methods.get(member).or_else(|| s.assoc_fns.get(member))
    }
}

fn resolve_enum_member<'p>(
    e: &'p crate::sema::typed::TypedEnum,
    member: &str,
) -> Option<&'p TypedFn> {
    e.methods.get(member).or_else(|| e.assoc_fns.get(member))
}

fn enum_by_name<'p>(
    prog: &'p TypedProgram,
    name: &str,
) -> Option<&'p crate::sema::typed::TypedEnum> {
    prog.enums
        .get(name)
        .or_else(|| prog.imported.enums.get(name))
}

fn resolve_struct<'p>(
    prog: &'p TypedProgram,
    name: &str,
    targs: &[TypeArg],
) -> Option<&'p TypedStruct> {
    if targs.is_empty() {
        struct_by_name(prog, name)
    } else {
        let key = generics::canonical_key(InstKind::Struct, name, targs);
        match instantiation_by_key(prog, &key) {
            Some(TypedInstantiation::Struct(s)) => Some(s),
            _ => None,
        }
    }
}

/// Plain declaration name a callee key hangs off — mirrors
/// `eval::interp::callee_decl_name` for the `unresolvable` lookup.
fn callee_decl_name(key: &CalleeKey) -> String {
    let raw = match key {
        CalleeKey::Fn(name) => name.clone(),
        CalleeKey::Method(sname, _) => sname.clone(),
        CalleeKey::FnInstance(k) | CalleeKey::MethodInstance(k, _) => k.clone(),
    };
    let no_prefix = raw
        .strip_prefix("fn:")
        .or_else(|| raw.strip_prefix("struct:"))
        .unwrap_or(&raw);
    no_prefix.split('[').next().unwrap_or(no_prefix).to_string()
}

/// Fail-closed miss for a Call/OpCall: prefer the import splice's own
/// `unresolvable` note (decision 15), else name the real cause. Never
/// blame "an unresolved generic instantiation" for a plain imported
/// name that simply was not wired into lower (plans/M9.md item EE).
fn missing_callee(prog: &TypedProgram, key: &CalleeKey) -> LowerError {
    let name = callee_decl_name(key);
    if let Some(note) = prog.imported.unresolvable.get(&name) {
        // Note is a complete sentence (`` `{name}` is declared… ``);
        // `named`, not `unimplemented` (plans/M9.md item D3).
        return LowerError::named(format!("`{name}` {note}"));
    }
    match key {
        CalleeKey::FnInstance(_) | CalleeKey::MethodInstance(_, _) => LowerError::unimplemented(
            "calling a callee not resolvable at lowering time (an unresolved generic instantiation)",
        ),
        _ => LowerError::unimplemented(format!(
            "calling `{}` — not declared in this module and not present in its import closure",
            key.spelling()
        )),
    }
}

/// Fail-closed miss for a struct layout/construction lookup. An
/// `internal error: struct \`X\` not found` reachable from ordinary
/// source is a bug by house rule; after EE the ordinary imported case
/// resolves, and a genuine miss names the import closure.
fn missing_struct(prog: &TypedProgram, name: &str) -> LowerError {
    if let Some(note) = prog.imported.unresolvable.get(name) {
        return LowerError::named(format!("`{name}` {note}"));
    }
    LowerError::unimplemented(format!(
        "struct `{name}` is not declared in this module and not present in its import closure"
    ))
}

fn field_index(prog: &TypedProgram, base_ty: &Type, field_name: &str) -> Result<usize, LowerError> {
    // plans/M9.md item C1: `String[..N].len` is slot 0.
    if matches!(base_ty, Type::String(_)) {
        return match field_name {
            "len" => Ok(0),
            other => Err(LowerError::internal(format!(
                "unknown String field `{other}`"
            ))),
        };
    }
    let Type::Named(sname, targs) = base_ty else {
        return Err(LowerError::internal("field base is not a `Named` type"));
    };
    // plans/M7.md item E4: IoCompletion is not a DeclStruct.
    if sname == "IoCompletion" {
        return match field_name {
            "payload" => Ok(0),
            "status" => Ok(1),
            "written_len" => Ok(2),
            other => Err(LowerError::internal(format!(
                "unknown IoCompletion field `{other}`"
            ))),
        };
    }
    let s = resolve_struct(prog, sname, targs).ok_or_else(|| missing_struct(prog, sname))?;
    s.fields
        .iter()
        .position(|f| f == field_name)
        .ok_or_else(|| LowerError::internal(format!("unknown field `{field_name}`")))
}

/// Materialize a `String[..N]` value as `MakeAggregate` of
/// `[len, b0, b1, …, b_{N-1}]` (padding unoccupied slots with 0).
fn emit_string_aggregate(
    bytes: &[u8],
    n_expr: &ast::Expr,
    ty: &Type,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    let n = eval_len_expr(n_expr)?;
    if bytes.len() > n {
        return Err(LowerError::internal(format!(
            "String literal of {} bytes exceeds capacity {n}",
            bytes.len()
        )));
    }
    let mut elems = Vec::with_capacity(1 + n);
    let len_t = b.fresh(Type::Usize);
    b.emit(Inst::ConstInt {
        dst: len_t,
        ty: Type::Usize,
        value: bytes.len() as i128,
    });
    elems.push(len_t);
    for i in 0..n {
        let byte_t = b.fresh(Type::U8);
        let v = i128::from(bytes.get(i).copied().unwrap_or(0));
        b.emit(Inst::ConstInt {
            dst: byte_t,
            ty: Type::U8,
            value: v,
        });
        elems.push(byte_t);
    }
    let dst = b.fresh(ty.clone());
    b.emit(Inst::MakeAggregate { dst, elems });
    Ok(dst)
}

/// Byte offset of field `index` inside `base_ty` — same walk codegen's
/// `field_offset_size` uses, so the live-cell address and the frame
/// layout can never disagree.
fn interrupt_cell_field_off(
    b: &FnBuilder,
    base_ty: &Type,
    index: usize,
) -> Result<usize, LowerError> {
    let Type::Named(sname, targs) = base_ty else {
        return Err(LowerError::internal(
            "InterruptCell field base is not a `Named` type",
        ));
    };
    let s =
        resolve_struct(b.prog(), sname, targs).ok_or_else(|| missing_struct(b.prog(), sname))?;
    // Builtin-pseudo-type fields (capabilities, `InterruptCell`, scalars,
    // `Option[...]`) size without a populated `LayoutCtx`. A nested user
    // struct field would need one; fail closed rather than guess.
    let layout = mwir::LayoutCtx::default();
    let mut off = 0usize;
    for (i, fname) in s.fields.iter().enumerate() {
        if i == index {
            return Ok(off);
        }
        let fty = s
            .field_types
            .get(fname)
            .cloned()
            .ok_or_else(|| LowerError::internal(format!("no type for field `{fname}`")))?;
        off += mwir::size_of(&fty, &layout).map_err(|e| {
            LowerError::unimplemented(&format!(
                "sizing field `{fname}` of `{sname}` for an `InterruptCell` live offset ({e}) is"
            ))
        })?;
    }
    Err(LowerError::internal(format!(
        "InterruptCell field index {index} out of range for `{sname}`"
    )))
}

/// plans/M7.md item G, decision 17: lower one `InterruptCell` intrinsic.
fn lower_interrupt_cell_intrinsic(
    key: &str,
    receiver: Option<&TypedExpr>,
    args: &[(String, TypedExpr)],
    ret_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    if key == "InterruptCell.new" {
        let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
            return Err(LowerError::internal(
                "`InterruptCell.new` with no value argument".to_string(),
            ));
        };
        let src = lower_expr(v, b, env)?;
        let dst = b.fresh(ret_ty.clone());
        b.emit(Inst::Copy { dst, src });
        return Ok(dst);
    }
    let Some(recv) = receiver else {
        return Err(LowerError::internal(format!(
            "`{key}` reached lowering with no receiver"
        )));
    };
    // Receiver must be `self.<field>` — the live cell lives in driver state.
    let TypedExprKind::Field(base, fname) = &recv.kind else {
        return Err(LowerError::unimplemented(
            "an `InterruptCell` op on a non-field place (only `self.<cell>` is supported) is",
        ));
    };
    let TypedExprKind::Local(base_name) = &base.kind else {
        return Err(LowerError::unimplemented(
            "an `InterruptCell` op through a nested field chain is",
        ));
    };
    if base_name != "self" {
        return Err(LowerError::unimplemented(
            "an `InterruptCell` op on a local cell (only a `@driver` field of `self` is live) is",
        ));
    }
    let base_ty = bodies::unwrap_own(base.ty.clone());
    let idx = field_index(b.prog(), &base_ty, fname)?;
    let field_off = interrupt_cell_field_off(b, &base_ty, idx)?;
    let width = 4u8; // `InterruptCell[u32]` only, today
    match key {
        "InterruptCell.load_acquire" => {
            let dst = b.fresh(ret_ty.clone());
            b.emit(Inst::InterruptCellLoadAcquire {
                dst,
                field_off,
                width,
            });
            Ok(dst)
        }
        "InterruptCell.store_release" => {
            let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                return Err(LowerError::internal(
                    "`InterruptCell.store_release` with no value".to_string(),
                ));
            };
            let value = lower_expr(v, b, env)?;
            b.emit(Inst::InterruptCellStoreRelease {
                field_off,
                width,
                value,
            });
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        "InterruptCell.swap_acquire" => {
            let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                return Err(LowerError::internal(
                    "`InterruptCell.swap_acquire` with no value".to_string(),
                ));
            };
            let value = lower_expr(v, b, env)?;
            let dst = b.fresh(ret_ty.clone());
            b.emit(Inst::InterruptCellSwapAcquire {
                dst,
                field_off,
                width,
                value,
            });
            Ok(dst)
        }
        "InterruptCell.fetch_or_release" => {
            let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                return Err(LowerError::internal(
                    "`InterruptCell.fetch_or_release` with no value".to_string(),
                ));
            };
            let value = lower_expr(v, b, env)?;
            let dst = b.fresh(ret_ty.clone());
            b.emit(Inst::InterruptCellFetchOrRelease {
                dst,
                field_off,
                width,
                value,
            });
            Ok(dst)
        }
        other => Err(LowerError::internal(format!(
            "unknown InterruptCell intrinsic `{other}`"
        ))),
    }
}

fn variant_index(prog: &TypedProgram, enum_name: &str, variant: &str) -> Result<usize, LowerError> {
    match enum_name {
        "Option" => match variant {
            "None" => Ok(value::OPTION_NONE),
            "Some" => Ok(value::OPTION_SOME),
            other => Err(LowerError::internal(format!(
                "unknown Option variant `{other}`"
            ))),
        },
        "Result" => match variant {
            "Ok" => Ok(value::RESULT_OK),
            "Err" => Ok(value::RESULT_ERR),
            other => Err(LowerError::internal(format!(
                "unknown Result variant `{other}`"
            ))),
        },
        _ => {
            // plans/M9.md item A1b / A2: this module's own enums, else the
            // ones it imports. Before this an *imported* enum (A2's
            // `IoError` from `stdlib/core/io_error.wr`) fell into the
            // "generic enum instantiation" rejection and named the wrong
            // cause — the same defect A1b already fixed in `eval::interp`.
            let en = prog
                .enums
                .get(enum_name)
                .or_else(|| prog.imported.enums.get(enum_name))
                .ok_or_else(|| {
                    LowerError::unimplemented(format!(
                        "constructing/matching a generic enum instantiation's variant (`{enum_name}.{variant}`) is",
                    ))
                })?;
            en.variants
                .iter()
                .position(|v| v == variant)
                .ok_or_else(|| {
                    LowerError::internal(format!("unknown variant `{enum_name}.{variant}`"))
                })
        }
    }
}

fn int_bits(ty: &Type) -> Result<u32, LowerError> {
    match ty {
        Type::U8 | Type::I8 => Ok(8),
        Type::U16 | Type::I16 => Ok(16),
        Type::U32 | Type::I32 => Ok(32),
        Type::U64 | Type::I64 | Type::Usize | Type::Isize => Ok(64),
        other => Err(LowerError::internal(format!(
            "shift on a non-integer type ({other:?})"
        ))),
    }
}

/// plans/M9.md item MM / F3: lower sealed `[T; N].map_take` /
/// `try_map_take`. N is compile-time known, so the walk unrolls — each
/// element is taken via `IndexGet`, passed `take` to the named mapper,
/// and collected into a `MakeAggregate`. `try_map_take` branches on each
/// `Result` tag; an `Err` becomes the whole expression's `Err` (data
/// reclaim is frame drop, matching the evaluator).
fn lower_array_map_take(
    key: &str,
    receiver: Option<&TypedExpr>,
    args: &[(String, TypedExpr)],
    result_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    let recv = receiver.ok_or_else(|| {
        LowerError::internal(format!(
            "`{key}` reached lowering without an array receiver"
        ))
    })?;
    let Type::Array(elem_ty, _) = bodies::unwrap_own(recv.ty.clone()) else {
        return Err(LowerError::internal(format!(
            "`{key}` receiver is not an array"
        )));
    };
    let len = eval_array_len(&recv.ty)?;
    let arr = lower_expr(recv, b, env)?;
    let (_, mapper_expr) = args.first().ok_or_else(|| {
        LowerError::internal(format!("`{key}` reached lowering without a mapper"))
    })?;
    let TypedExprKind::FnRef(mapper_key) = &mapper_expr.kind else {
        return Err(LowerError::unimplemented(
            "`Array.map_take` / `Array.try_map_take` with a non-named-fn mapper is",
        ));
    };
    let mapper_spelling = mapper_key.spelling();
    let is_try = key == "Array.try_map_take";

    if !is_try {
        let Type::Array(out_elem, _) = result_ty else {
            return Err(LowerError::internal(
                "`Array.map_take` result is not an array",
            ));
        };
        let mut outs = Vec::with_capacity(len);
        for i in 0..len {
            let idx_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: idx_t,
                ty: Type::Usize,
                value: i as i128,
            });
            let elem_t = b.fresh((*elem_ty).clone());
            b.emit(Inst::IndexGet {
                dst: elem_t,
                base: arr,
                index: idx_t,
                len,
            });
            let out_t = b.fresh((**out_elem).clone());
            b.emit(Inst::Call {
                dst: out_t,
                write_backs: vec![],
                key: mapper_spelling.clone(),
                args: vec![elem_t],
            });
            outs.push(out_t);
        }
        let dst = b.fresh(result_ty.clone());
        b.emit(Inst::MakeAggregate { dst, elems: outs });
        return Ok(dst);
    }

    // try_map_take: Result[[U; N], E]
    let Type::Result(ok_arr_ty, err_ty) = result_ty else {
        return Err(LowerError::internal(
            "`Array.try_map_take` result is not a Result",
        ));
    };
    let Type::Array(out_elem, _) = ok_arr_ty.as_ref() else {
        return Err(LowerError::internal(
            "`Array.try_map_take` Ok payload is not an array",
        ));
    };
    let result = b.fresh(result_ty.clone());
    let mut ok_elems = Vec::with_capacity(len);
    let mut err_entry_fixups: Vec<(usize, Temp)> = Vec::new();
    for i in 0..len {
        let idx_t = b.fresh(Type::Usize);
        b.emit(Inst::ConstInt {
            dst: idx_t,
            ty: Type::Usize,
            value: i as i128,
        });
        let elem_t = b.fresh((*elem_ty).clone());
        b.emit(Inst::IndexGet {
            dst: elem_t,
            base: arr,
            index: idx_t,
            len,
        });
        let mapped_ty = Type::Result(Box::new((**out_elem).clone()), Box::new((**err_ty).clone()));
        let mapped = b.fresh(mapped_ty);
        b.emit(Inst::Call {
            dst: mapped,
            write_backs: vec![],
            key: mapper_spelling.clone(),
            args: vec![elem_t],
        });
        let tag_t = b.fresh(Type::U64);
        b.emit(Inst::EnumTag {
            dst: tag_t,
            src: mapped,
        });
        let ok_const = b.fresh(Type::U64);
        b.emit(Inst::ConstInt {
            dst: ok_const,
            ty: Type::U64,
            value: value::RESULT_OK as i128,
        });
        let is_ok = b.fresh(Type::Bool);
        b.emit(Inst::Compare {
            dst: is_ok,
            op: BinOp::Eq,
            ty: Type::U64,
            lhs: tag_t,
            rhs: ok_const,
        });
        let err_fixup = b.emit(Inst::JumpIfFalse {
            cond: is_ok,
            target: usize::MAX,
        });
        err_entry_fixups.push((err_fixup, mapped));
        let payload = b.fresh((**out_elem).clone());
        b.emit(Inst::EnumPayload {
            dst: payload,
            src: mapped,
            index: 0,
        });
        ok_elems.push(payload);
    }
    let arr_out = b.fresh((**ok_arr_ty).clone());
    b.emit(Inst::MakeAggregate {
        dst: arr_out,
        elems: ok_elems,
    });
    b.emit(Inst::MakeEnum {
        dst: result,
        tag: value::RESULT_OK,
        payload: vec![arr_out],
    });
    let success_end_fixup = b.emit(Inst::Jump { target: usize::MAX });

    let mut err_end_fixups = Vec::new();
    for (entry_fixup, mapped) in err_entry_fixups {
        let err_pos = b.here();
        b.patch_jump(entry_fixup, err_pos);
        let err_payload = b.fresh((**err_ty).clone());
        b.emit(Inst::EnumPayload {
            dst: err_payload,
            src: mapped,
            index: 0,
        });
        b.emit(Inst::MakeEnum {
            dst: result,
            tag: value::RESULT_ERR,
            payload: vec![err_payload],
        });
        let j = b.emit(Inst::Jump { target: usize::MAX });
        err_end_fixups.push(j);
    }
    let end_pos = b.here();
    b.patch_jump(success_end_fixup, end_pos);
    for j in err_end_fixups {
        b.patch_jump(j, end_pos);
    }
    Ok(result)
}

/// `base`'s own array length, resolved at lowering time — a literal, or
/// a plain module `const` reference (module doc's own fail-closed
/// enumeration covers anything else, and `Bytes`).
fn eval_array_len(ty: &Type) -> Result<usize, LowerError> {
    match ty {
        Type::Array(_, len_expr) => eval_len_expr(len_expr),
        Type::Own(_, inner) => eval_array_len(inner),
        _ => Err(LowerError::unimplemented(
            "indexing a non-array (e.g. `Bytes`) value is",
        )),
    }
}

fn eval_len_expr(e: &ast::Expr) -> Result<usize, LowerError> {
    if let Some(n) = bodies::literal_array_len(e) {
        return usize::try_from(n).map_err(|_| LowerError::internal("array length out of range"));
    }
    Err(LowerError::unimplemented(
        "an array length expression that is not a literal is",
    ))
}

// --- unit tests -----------------------------------------------------------

#[cfg(test)]
mod builder_tests {
    use super::*;

    fn new_lowerer(prog: &TypedProgram) -> Lowerer<'_> {
        Lowerer {
            prog,
            rodata: Vec::new(),
            rodata_index: BTreeMap::new(),
        }
    }

    // plans/M5.md item B, task note 6: "label resolution" — a direct,
    // builder-level test of the backpatch mechanism every structured
    // construct (`if`/`while`/`for`/`match`/`and`/`or`) relies on: a
    // forward jump emitted with a placeholder must end up pointing at
    // the exact instruction index reached once patched.
    #[test]
    fn patch_jump_resolves_a_forward_jump_to_the_patched_index() {
        let prog = TypedProgram::default();
        let mut lw = new_lowerer(&prog);
        let mut b = FnBuilder {
            lw: &mut lw,
            temp_types: Vec::new(),
            body: Vec::new(),
            ret: Type::Unit,
            owner_struct: None,
        };
        let cond = b.fresh(Type::Bool);
        b.emit(Inst::ConstBool {
            dst: cond,
            value: true,
        });
        let fixup = b.emit(Inst::JumpIfFalse {
            cond,
            target: usize::MAX,
        });
        b.emit(Inst::ConstUnit { dst: cond });
        b.emit(Inst::ConstUnit { dst: cond });
        let target = b.here();
        b.patch_jump(fixup, target);
        match &b.body[fixup] {
            Inst::JumpIfFalse { target: t, .. } => assert_eq!(*t, target),
            other => panic!("expected JumpIfFalse, got {other:?}"),
        }
        assert_eq!(target, 4);
    }

    #[test]
    fn rodata_interning_dedupes_identical_bytes() {
        let prog = TypedProgram::default();
        let mut lw = new_lowerer(&prog);
        let mut b = FnBuilder {
            lw: &mut lw,
            temp_types: Vec::new(),
            body: Vec::new(),
            ret: Type::Unit,
            owner_struct: None,
        };
        let a = b.intern(b"hello".to_vec());
        let c = b.intern(b"world".to_vec());
        let d = b.intern(b"hello".to_vec());
        assert_eq!(a, d);
        assert_ne!(a, c);
        assert_eq!(b.lw.rodata.len(), 2);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::sema;
    use crate::syntax::{lexer, parser};

    fn typed_program(src: &str) -> TypedProgram {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check_typed(&module, "<test>").expect("test source must check")
    }

    #[test]
    fn runtime_force_root_seeds_probe_when_runtime_loaded() {
        // plans/M10.md item A2d / decision 583: a `@test(runtime)` root
        // with auto-loaded `core.runtime` force-roots the probe into the
        // reachable set even though no Call names it.
        let src = "\
module examples.force_root_probe

@test(runtime)
pub fn t():
    return
";
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("runtime.wr must load"),
        };
        let root_key = module.path.clone();
        let mut modules = BTreeMap::new();
        modules.insert(root_key.clone(), module);
        modules.insert(runtime_key.clone(), runtime_loaded.module);
        let mut paths = BTreeMap::new();
        paths.insert(root_key.clone(), "<test>".to_string());
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        let programs = sema::check_program_typed(&modules, &paths).expect("check");
        let by_dot: BTreeMap<String, TypedProgram> = programs
            .into_iter()
            .map(|(k, p)| (k.join("."), p))
            .collect();
        let reachable = guest_reachable_keys_closure(&by_dot, &LowerOpts::default());
        assert!(
            reachable.contains("__wrela_runtime_probe"),
            "force-root must seed the probe: {reachable:?}"
        );
        let lower_opts = LowerOpts {
            emit_comptime_tests: false,
            only: Some(reachable),
        };
        let mut saw_probe = false;
        for typed in by_dot.values() {
            let mwir = lower_program_with(typed, &lower_opts).expect("lower");
            if mwir.fns.contains_key("__wrela_runtime_probe") {
                saw_probe = true;
            }
        }
        assert!(saw_probe, "probe must lower from core.runtime");
    }

    #[test]
    fn lowers_a_plain_arithmetic_fn() {
        let program = typed_program(
            "module examples.lower_arith

pub fn add_one(x: u64) -> u64:
    return x + 1
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let f = mwir.fns.get("add_one").expect("fn lowered");
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::ArithChecked { .. }))
        );
        assert!(matches!(f.body.last(), Some(Inst::Return { .. })));
    }

    #[test]
    fn lowers_a_struct_method_and_init() {
        let program = typed_program(
            "module examples.lower_struct

pub struct Counter:
    value: u64

    init(mut self, start: u64):
        self.value = start

    pub fn bump(mut self, by: u64):
        self.value = self.value + by

pub fn use_counter() -> u64:
    c = Counter(start=10)
    c.bump(5)
    return c.value
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        assert!(mwir.fns.contains_key("Counter.init"));
        assert!(mwir.fns.contains_key("Counter.bump"));
        let use_fn = mwir.fns.get("use_counter").expect("fn lowered");
        assert!(use_fn.body.iter().any(|i| matches!(
            i,
            Inst::Call { key, .. } if key == "Counter.init"
        )));
        assert!(use_fn.body.iter().any(|i| matches!(
            i,
            Inst::Call { key, write_backs, .. } if key == "Counter.bump" && !write_backs.is_empty()
        )));
    }

    #[test]
    fn lowers_an_enum_match() {
        let program = typed_program(
            "module examples.lower_match

pub enum Shape:
    Circle(u64)
    Square(u64)

pub fn area(s: Shape) -> u64:
    match s:
        case .Circle(r):
            return r * r
        case .Square(side):
            return side * side
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let f = mwir.fns.get("area").expect("fn lowered");
        assert!(f.body.iter().any(|i| matches!(i, Inst::EnumTag { .. })));
        assert!(f.body.iter().any(|i| matches!(i, Inst::EnumPayload { .. })));
        assert!(f.body.iter().any(|i| matches!(i, Inst::AssertFail { .. })));
    }

    #[test]
    fn lowers_a_loop_with_an_accumulator() {
        let program = typed_program(
            "module examples.lower_loop

pub fn sum_to(n: u64) -> u64:
    total: u64 = 0
    i: u64 = 0
    while i < n:
        total = total + i
        i = i + 1
    return total
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let f = mwir.fns.get("sum_to").expect("fn lowered");
        assert!(f.body.iter().any(|i| matches!(i, Inst::Jump { .. })));
        assert!(f.body.iter().any(|i| matches!(i, Inst::JumpIfFalse { .. })));
    }

    #[test]
    fn a_generic_fn_instantiation_lowers_under_its_own_key() {
        let program = typed_program(
            "module examples.lower_generic

pub fn identity[T](x: T) -> T:
    return x

pub fn use_identity() -> i64:
    return identity(42)
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        assert!(
            mwir.fns.keys().any(|k| k.starts_with("fn:identity")),
            "expected an instantiated `identity` key, got {:?}",
            mwir.fns.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn take_lowers_transparently_with_no_extra_instruction() {
        let program = typed_program(
            "module examples.lower_take

pub fn consume(take x: u64) -> u64:
    return x

pub fn use_it() -> u64:
    a: u64 = 7
    return consume(take a)
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        assert!(mwir.fns.contains_key("consume"));
        assert!(mwir.fns.contains_key("use_it"));
    }

    #[test]
    fn closures_fail_closed() {
        let program = typed_program(
            "module examples.lower_closure

pub fn apply_twice(f: fn(u64) -> u64, x: u64) -> u64:
    return f(f(x))

const RESULT: u64 = apply_twice(|v: u64| v * 2, 3)
",
        );
        let err = lower_program(&program).expect_err("closures must fail closed");
        assert!(err.message.contains("closure"), "message: {}", err.message);
    }

    // `sema::bodies::check_assert` requires `assert`'s own message to be
    // a text *literal* — so a non-literal message can never reach this
    // pass through `assert`; `panic(msg)`'s own message, though, only
    // needs to type-check as `Static[Str]` (`check_call_by_name`'s own
    // `"panic"` arm), which a non-literal expression satisfies just
    // fine. This is the one construct this item's own required err
    // golden pins: it passes `check_typed` but this pass cannot lower it
    // (module doc's own fail-closed enumeration).
    #[test]
    fn a_non_literal_panic_message_fails_closed() {
        let program = typed_program(
            "module examples.lower_dynamic_panic

pub fn label() -> Static[Str]:
    return \"nope\"

pub fn check():
    panic(label())
",
        );
        let err =
            lower_program(&program).expect_err("a non-literal panic message must fail closed");
        assert!(
            err.message.contains("non-literal"),
            "message: {}",
            err.message
        );
    }

    /// plans/M7.md item H1 self-audit: `mmio_register_offset`'s cross-
    /// module arm and the `None`-register internal. The cross-module
    /// case is not source-reachable from a single-module golden (the
    /// build closure would need a layout in another module that this
    /// module's `TypedProgram::layouts` does not carry); pinned by
    /// constructing a program whose layouts table is empty.
    #[test]
    fn mmio_register_offset_fail_closed_arms() {
        let empty = TypedProgram::default();
        let cross =
            mmio_register_offset("ForeignRegs", "status", &empty).expect_err("cross-module");
        assert!(
            cross.message.contains("different module") && cross.message.contains("ForeignRegs"),
            "{}",
            cross.message
        );

        // A layout is present but the register is not — the checker
        // already refused this, so it is an `internal`.
        let mut prog = TypedProgram::default();
        prog.layouts.push(crate::sema::types::LayoutType {
            name: "Regs".to_string(),
            kind: crate::sema::types::LayoutKind::Mmio,
            endian: crate::sema::types::LayoutEndian::Little,
            size: Some(4),
            padding: 0,
            entries: vec![],
        });
        let missing = mmio_register_offset("Regs", "nope", &prog).expect_err("missing reg");
        assert!(
            missing.message.contains("internal error:")
                && missing.message.contains("declares no register `nope`"),
            "{}",
            missing.message
        );
    }

    /// plans/M7.md item H1 self-audit: `mmio_access_names`' defensive
    /// internals — each is unreachable through a checked program
    /// (`check_mmio_access` already shapes the node), kept as named
    /// rejections rather than panics.
    #[test]
    fn mmio_access_names_internal_guards() {
        let not_mmio = mmio_access_names(&Type::U32, &[]).expect_err("not Mmio");
        assert!(
            not_mmio.message.contains("not an `Mmio[L]`"),
            "{}",
            not_mmio.message
        );

        let wrong_cap = mmio_access_names(&Type::Named("DeviceCap".to_string(), vec![]), &[])
            .expect_err("DeviceCap");
        assert!(
            wrong_cap.message.contains("not an `Mmio[L]`"),
            "{}",
            wrong_cap.message
        );

        let bad_targ = mmio_access_names(
            &Type::Named(
                "Mmio".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::U32)],
            ),
            &[],
        )
        .expect_err("non-named layout arg");
        assert!(
            bad_targ
                .message
                .contains("layout argument is not a named type"),
            "{}",
            bad_targ.message
        );

        let no_reg = mmio_access_names(
            &Type::Named(
                "Mmio".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Named(
                    "Regs".to_string(),
                    vec![],
                ))],
            ),
            &[],
        )
        .expect_err("no register");
        assert!(
            no_reg.message.contains("no register name"),
            "{}",
            no_reg.message
        );

        let non_lit = mmio_access_names(
            &Type::Named(
                "Mmio".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Named(
                    "Regs".to_string(),
                    vec![],
                ))],
            ),
            &[(
                "register".to_string(),
                TypedExpr {
                    ty: Type::Named(
                        "Static".to_string(),
                        vec![crate::sema::types::TypeArg::Type(Type::Named(
                            "Str".to_string(),
                            vec![],
                        ))],
                    ),
                    kind: TypedExprKind::Local("r".to_string()),
                },
            )],
        )
        .expect_err("non-literal register");
        assert!(
            non_lit.message.contains("register name is not a literal"),
            "{}",
            non_lit.message
        );
    }
}
