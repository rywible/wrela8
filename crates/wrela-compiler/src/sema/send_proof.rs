//! The `send` proof (plans/M6.md item G, decision 5; 02-language.md
//! §9.4): "When mailbox analysis proves admission cannot fail (including
//! during a restart window), the error type is `never` and `send` stands
//! as a bare statement; otherwise the result must be consumed. ... This
//! is the language's one proof-conditioned form: the same spelling is
//! infallible exactly where the compiler has proved it."
//!
//! ## The shipped condition, verbatim (plans/M6.md decision 5)
//!
//! > admission cannot fail iff the target mailbox's declared capacity ≥
//! > the whole-image count of static send/call sites targeting it AND
//! > every such site sits outside any loop and outside any `g.start`
//! > child (so each executes at most once per root turn) AND root turns
//! > are serial (decision 1 guarantees this at M6).
//!
//! Everything below is that sentence, made mechanical. The one place
//! this module is *stricter* than the sentence's own parenthetical:
//! "outside any loop and outside any `g.start` child" is necessary for
//! "each executes at most once per root turn" but not sufficient — a
//! site in a fn called from two places, or from inside a loop in its
//! caller, or reachable through a recursive cycle, executes more than
//! once while sitting outside every loop *in its own body*. Decision 5's
//! stated intent is the parenthetical, so this module implements the
//! intent (`at_most_once`, below) and treats the two lexical tests as
//! what they are: the first two of its conditions, not all of them.
//! Over-counting is safe; under-counting would let a real image drop a
//! message, which is the one failure mode this analysis exists to
//! prevent.
//!
//! ## Where this runs, and why
//!
//! At the *end* of `sema::check_typed`/`check_program_typed`, over every
//! module in the build closure at once — not inside any body-checking
//! pass, and not in `eval::image_checks`.
//!
//!   - Not in a body pass: a mailbox capacity is a *declared* value in
//!     the `@image` fn (`img.actor(A, mailbox=N)`), which only exists
//!     once the whole-image builder has been evaluated. `bodies.rs` has
//!     no `ImageGraph` and cannot get one (it is running before the
//!     program it would evaluate is finished).
//!   - Not in `eval::image_checks::check_sealed`: that pass only runs
//!     from `--stage=image`/`report`/`build`, so a bare `send` would
//!     type clean at `--stage=check`/`typed` and only be rejected two
//!     stages later — and a program with **no** `@image` fn at all
//!     (which can prove nothing, and whose bare sends must therefore be
//!     rejected) never reaches that pass in the first place. Fail
//!     closed: the rejection has to live where every consumer of the
//!     checked program passes through it.
//!
//! So this pass evaluates the one reachable `@image` fn itself
//! (`eval::interp::eval_image` — the same entry `--stage=image` uses),
//! and only when the closure actually contains a bare `send` statement:
//! a program without one never pays for an extra evaluation and never
//! changes behaviour by one byte. The `@image` fn is evaluated a second
//! time later by `--stage=image`/`report`/`build`; that is the honest
//! cost of the placement, and it is a pure function of the typed program
//! (deterministic, quota-bounded), so the two evaluations can never
//! disagree.
//!
//! ## The counting rule, precisely
//!
//! A **message site** is any typed `send h.m(...)` or `await h.m(...)`
//! whose receiver is an `Actor[T]` handle — both enqueue one message
//! into `T`'s single mailbox and occupy one slot there, so both count
//! (decision 5's own "send/call sites"). The site is attributed to the
//! actor *type* `T`, never to a particular instance: a handle's static
//! type is all the compiler knows, so an image with two `Foo` instances
//! could route every site to whichever one it likes. `capacity(T)` is
//! therefore the **minimum** declared `mailbox=` over every declared
//! instance of `T`, and `N(T)` the count of *every* message site
//! targeting `T` anywhere in the build closure.
//!
//! Multiplicity, i.e. "how many times can this site execute per root
//! turn", is over-approximated by `at_most_once`:
//!
//!   - a site lexically inside a `while`/`for` body, or inside a closure
//!     body (a closure is not a graph node — it may be applied any
//!     number of times), is **not** at-most-once;
//!   - otherwise the site inherits its *holder*'s verdict — the fn,
//!     method or instantiation whose body it sits in:
//!       - a holder reachable from a `g.start` callee through ordinary
//!         call edges is **not** at-most-once (decision 5's own
//!         "outside any `g.start` child", read transitively; a message
//!         edge is deliberately not an ordinary call edge — an actor
//!         turn started by a message is not lexically inside the child's
//!         frame);
//!       - a holder that is its own transitive caller (recursion, direct
//!         or mutual) is **not** at-most-once;
//!       - a holder with zero static invocation sites is at-most-once —
//!         it either never runs at all, or it is a root (`@test(runtime)`
//!         is the only root at M6) and decision 1 makes root turns
//!         serial, one at a time;
//!       - a holder with exactly one static invocation site is
//!         at-most-once iff that site is;
//!       - a holder with two or more static invocation sites is **not**
//!         at-most-once.
//!
//! An *invocation site* of a holder key is any typed callee occurrence
//! naming it — an ordinary `Call`/`FnRef`/`OpCall`/`Try`-conversion, a
//! `g.start` callee argument, or a message site (an actor method's own
//! invocations *are* the messages sent to it, which is exactly the
//! uniformity that makes this one rule cover actor methods too). Keys
//! are `CalleeKey::spelling()`s, merged across modules: two modules that
//! each declare `fn helper` share one key here, so their call sites add
//! up — conservative in the safe direction, recorded rather than fixed
//! (module-qualified keys are not what the typed tree carries).
//!
//! Anything this pass cannot see contributes no *edge* but also no
//! proof: a holder with an unknown caller looks like a holder with zero
//! callers, which is at-most-once — sound only because the sites the
//! unknown caller could reach were themselves already counted into
//! `N(T)` by the whole-closure scan. `N(T)` is a count of static source
//! sites, not of dynamic paths, so no call graph can add a site to it.
//!
//! ## Why no under-count is possible
//!
//! Every message into `T`'s mailbox originates at some static message
//! site in the build closure — there is no other spelling that enqueues
//! (`bodies::check_send`/`check_await` are the only producers of
//! `TypedExprKind::Send`/`Await`-of-actor-call, and `codegen::emit_send`/
//! the await glue are the only `rt_enqueue` callers). The scan below
//! walks every statement and expression of every fn, method, `init` and
//! instantiation of every module in the closure, exhaustively (a new
//! node kind stops this file compiling until it gets a real arm), so
//! `N(T)` counts them all. Each site is required to be at-most-once per
//! root turn, so at most `N(T)` messages exist per root turn; root turns
//! are serial and each drains the scheduler before the next begins
//! (decision 1), so no two root turns' messages are ever live together;
//! `capacity(T) >= N(T)` therefore leaves a free slot for every one of
//! them and `rt_enqueue` can never reject.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::image::ImageGraph;
use crate::sema::SemaError;
use crate::sema::typed::{
    CalleeKey, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types;
use crate::syntax::ast::Span;

// --- collected facts -------------------------------------------------------

/// One `send`/`await` through an `Actor[T]` handle.
#[derive(Debug, Clone)]
struct MessageSite {
    /// The target actor struct's own name (`T` in `Actor[T]`).
    actor: String,
    /// The message method's own name — diagnostics only.
    method: String,
    /// The fn/method/instantiation key whose body this site sits in.
    holder: String,
    /// `Some(span)` exactly for a bare `send` **statement** (the form
    /// this whole pass exists to judge); `None` for the consumed
    /// expression form and for every `await`.
    bare: Option<Span>,
    /// Is this site outside every loop and closure body *within its own
    /// holder's body*? (The holder's own execution count is a separate
    /// question — `at_most_once`.)
    once_locally: bool,
}

/// One static invocation of some callee key.
#[derive(Debug, Clone)]
struct CallOccurrence {
    holder: String,
    once_locally: bool,
    /// The invoking expression's own span — the site
    /// `check_sync_call_graph_acyclic` points its diagnostic at when this
    /// occurrence turns out to close a recursive cycle.
    span: Span,
}

#[derive(Debug, Default)]
struct Facts {
    sites: Vec<MessageSite>,
    /// callee key -> every static invocation of it in the closure.
    callers: BTreeMap<String, Vec<CallOccurrence>>,
    /// Ordinary (non-message) call edges: holder key -> callee keys.
    /// Used only for the `g.start`-child reachability closure.
    edges: BTreeMap<String, BTreeSet<String>>,
    /// The strictly narrower **frame-extending** subset of `edges`: holder
    /// key -> callees whose body runs on the holder's own stack, which is
    /// the only graph `check_sync_call_graph_acyclic` may reason about.
    ///
    /// `edges` is deliberately wider — it exists for `g.start`-child
    /// reachability, where a mere *mention* of a callee is the point — and
    /// two of the occurrences it records do not extend anybody's frame:
    ///
    ///   - a bare `FnRef`, which is a reference and not a call. `wake(f)`
    ///     is spelled this way, and a `@task` bottom half that re-arms
    ///     itself with `wake(Self.drain)` (03-hardware.md §6, exactly
    ///     `golden/check-interrupt-cell`) is a *scheduling* self-edge: the
    ///     `wake` returns immediately and the task runs later, on its own
    ///     stack. Counting it as recursion would refuse the level-drain
    ///     idiom the hardware chapter prescribes.
    ///   - a `GroupChild`, i.e. the callee argument of `g.start(f)`, which
    ///     starts a child task with its own frame rather than descending
    ///     into the parent's.
    ///
    /// Message edges (`send`/`await` to an actor) were never in `edges` to
    /// begin with, and are excluded here for the same reason.
    sync_edges: BTreeMap<String, BTreeSet<String>>,
    /// Every key named as a `g.start` callee argument anywhere.
    group_children: BTreeSet<String>,
}

// --- the public entry ------------------------------------------------------

/// Judges every bare `send` statement in the build closure. `programs`
/// is every checked module (module path -> its typed program), in
/// deterministic order; the first bare `send` (module order, then holder
/// key order, then source order) that cannot be proven wins the
/// diagnostic, matching sema's own fail-fast discipline everywhere else.
pub(crate) fn check(programs: &BTreeMap<String, &TypedProgram>) -> Result<(), SemaError> {
    let facts = collect(programs);
    // 04-compiler.md §1's memory obligation, second half (see
    // `check_sync_call_graph_acyclic`). Runs first and unconditionally:
    // unlike the bare-`send` proof below it needs no `@image` fn and no
    // mailbox capacity, only the call edges `collect` just built.
    check_sync_call_graph_acyclic(&facts, programs)?;
    if !facts.sites.iter().any(|s| s.bare.is_some()) {
        // Nothing to prove: no `@image` evaluation, no behaviour change,
        // no cost. Every program without a bare `send` statement takes
        // this path.
        return Ok(());
    }

    let capacities = actor_capacities(programs);
    let in_child = group_child_closure(&facts);
    let mut memo: BTreeMap<String, bool> = BTreeMap::new();

    for site in &facts.sites {
        let Some(span) = site.bare else { continue };
        if let Some(reason) = unprovable_reason(site, &facts, &capacities, &in_child, &mut memo) {
            return Err(rejection(site, span, reason));
        }
    }
    Ok(())
}

// --- the recursion rejection (04-compiler.md §1 / 01-model.md §5) ---------

/// Rejects every cycle in the **synchronous** call graph — 04-compiler.md
/// §1's "unbounded recursion in either the sync or async call graph is
/// rejected", and 01-model.md §5's safety claim that wrela "prevents ...
/// unbounded runtime allocation and recursion".
///
/// This lived nowhere until an adversarial audit went looking for it. The
/// sibling obligation in the *same sentence* — "task frames, stacks, pools
/// ... have proven bounds" — was enforced (`eval::observes::
/// check_loop_discharge` refuses a `while` without `@budget`), but
/// `fn down(n: u64) -> u64: ... return down(n - 1)` checked clean and
/// lowered to real A64. Mutual recursion likewise.
///
/// **Why this is a memory-safety rule and not a tidiness rule.** Nothing
/// in the emitted code or the machine catches a blown stack. Prologues are
/// a bare `sub sp, sp, #N` with no limit compare; `wrela-vmm`'s
/// `boot_image_core` maps the whole 1 GiB DRAM reservation RW in one
/// `hv_vm_map` and only raises declared *exec* sections to RX, so no page
/// in a stack region is unmapped and there is no guard page to fault on;
/// and `wrela_machine::layout::core_stack_base_n` packs the per-core 1 MiB
/// stacks **contiguously** down from `DRAM_END`, so core `n`'s stack floor
/// is exactly core `n-1`'s stack ceiling. A runaway recursion on core 1
/// therefore walks SP straight down into core 0's live frames and silently
/// corrupts another core's actor state — no fault, no abort, a green boot
/// with wrong answers. That is precisely the "cross-actor shared mutable
/// state" 01-model.md §5 claims to prevent.
///
/// **Scope: ordinary call edges only.** `Facts::edges` records exactly the
/// `ordinary` invocations (`Call`/`FnRef`/`OpCall`/`Try`-conversion/
/// `g.start` callee) and deliberately excludes message edges, which is the
/// right graph here for the same reason it is the right graph for
/// `group_child_closure`: a `send`/`await` to an actor does not extend the
/// caller's frame — the message is admitted to a mailbox and run as its own
/// turn on a fresh stack. A message cycle is a *mailbox* bound, not a stack
/// bound, and it already has its own proof (`check`, above, plus
/// `reserve_proof`). Rejecting message cycles here would refuse ordinary
/// request/response protocols between two actors, which the language
/// plainly intends to allow.
///
/// The graph is over `CalleeKey::spelling()`s merged across modules, the
/// same keys `at_most_once` uses — so, exactly as documented there, two
/// modules that each declare `fn helper` share one node. That over-counts
/// edges and can only ever make this check *stricter*, never laxer, which
/// is the safe direction for a rule whose failure mode is silent memory
/// corruption.
///
/// **Scope: cycles reachable from a runtime entry point** (`runtime_roots`,
/// below). This boundary is not a hedge — it is what keeps the rule from
/// swallowing a different, deliberately-supported feature. Comptime
/// evaluation runs in `eval::interp`, not on any guest stack, and it is
/// bounded by its own `MAX_CALL_DEPTH = 1_000` quota (02-language.md §12,
/// `comptime.eval.quotas`); `eval::legal::classify` says so outright —
/// "recursion/cycles are legal by decision 7 (quotas bound them at eval
/// time)" — and has unit tests pinning it. A `const` initialized by a
/// recursive helper is therefore *already* bounded by a different
/// mechanism, and rejecting it here would delete a documented capability
/// to fix a hazard it does not have. Only a cycle some turn, test, or ISR
/// can actually enter puts frames on the guest stack, and that is exactly
/// what this rejects.
fn check_sync_call_graph_acyclic(
    facts: &Facts,
    programs: &BTreeMap<String, &TypedProgram>,
) -> Result<(), SemaError> {
    let reachable = runtime_reachable(facts, programs);
    if reachable.is_empty() {
        return Ok(());
    }
    /// Iterative DFS colouring: `Grey` is "on the current path", `Black`
    /// is "fully explored, no cycle reachable". A back edge to a `Grey`
    /// node is a cycle. Deterministic by construction — `edges` is a
    /// `BTreeMap` of `BTreeSet`s, so roots and neighbours are both walked
    /// in sorted order and the *same* cycle wins every run.
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        Grey,
        Black,
    }
    let mut colour: BTreeMap<&str, Colour> = BTreeMap::new();
    // The current DFS path, as node keys; `path[i+1]` is a callee of
    // `path[i]`. Reconstructing the cycle from this is what lets the
    // diagnostic name the whole loop rather than just one function.
    let mut path: Vec<&str> = Vec::new();

    // `Step::Enter` visits a node; `Step::Leave` pops it off the path and
    // blackens it. An explicit stack rather than recursion, because a
    // *recursive* cycle detector would itself blow the host stack on a
    // deep call graph — the exact bug class this function exists to reject.
    enum Step<'a> {
        Enter(&'a str),
        Leave(&'a str),
    }

    for root in facts.sync_edges.keys() {
        if colour.contains_key(root.as_str()) || !reachable.contains(root.as_str()) {
            continue;
        }
        let mut work: Vec<Step> = vec![Step::Enter(root.as_str())];
        while let Some(step) = work.pop() {
            match step {
                Step::Leave(key) => {
                    colour.insert(key, Colour::Black);
                    debug_assert_eq!(path.last().copied(), Some(key));
                    path.pop();
                }
                Step::Enter(key) => {
                    match colour.get(key) {
                        Some(Colour::Black) => continue,
                        Some(Colour::Grey) => {
                            // Back edge: `path` currently holds the cycle
                            // from `key`'s first occurrence to its end,
                            // and the caller closing it is `path.last()`.
                            let from = path.last().copied().unwrap_or(key);
                            let start = path.iter().position(|n| *n == key).unwrap_or(0);
                            let mut cycle: Vec<&str> = path[start..].to_vec();
                            cycle.push(key);
                            return Err(recursion_rejection(facts, from, key, &cycle));
                        }
                        None => {}
                    }
                    colour.insert(key, Colour::Grey);
                    path.push(key);
                    work.push(Step::Leave(key));
                    if let Some(callees) = facts.sync_edges.get(key) {
                        // Reverse so the sorted order is what actually
                        // gets explored first off the LIFO stack.
                        for callee in callees.iter().rev() {
                            if colour.get(callee.as_str()) != Some(&Colour::Black)
                                && reachable.contains(callee.as_str())
                            {
                                work.push(Step::Enter(callee.as_str()));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Every call-graph node a **runtime** entry point can reach through
/// frame-extending calls. The roots are the places guest execution
/// actually begins (04-compiler.md §2's per-core loops and the test
/// harness):
///
///   - every `@test(runtime)` fn — `wrela test`'s own roots;
///   - every method, associated fn and `init` of an `@actor` or `@driver`
///     struct — an actor turn and a driver entry both start here;
///   - every `@task` fn — a bottom half runs on its own stack, but that
///     stack is still a guest stack.
///
/// Anything outside this closure is either dead in this build or reached
/// only from a `const`/`@image` position, which `eval::interp` evaluates
/// under its own call-depth quota rather than on a guest stack.
///
/// An empty set means the closure has no runtime entry at all (a pure
/// library or a check-only fixture), and the caller returns early: there
/// is no guest stack to overflow yet, and the rejection lands as soon as
/// something roots the cycle.
fn runtime_reachable(
    facts: &Facts,
    programs: &BTreeMap<String, &TypedProgram>,
) -> BTreeSet<String> {
    let mut work: Vec<String> = Vec::new();
    for program in programs.values() {
        for t in &program.tests {
            if matches!(t.kind, crate::sema::typed::TestKind::Runtime) {
                work.push(t.name.clone());
            }
        }
        for (name, f) in &program.fns {
            if f.is_task {
                work.push(name.clone());
            }
        }
        for (struct_name, st) in &program.structs {
            if !st.is_actor && !st.is_driver {
                continue;
            }
            for member in st.methods.keys().chain(st.assoc_fns.keys()) {
                work.push(format!("{struct_name}.{member}"));
            }
            if st.init.is_some() {
                work.push(format!("{struct_name}.init"));
            }
        }
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(key) = work.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(callees) = facts.sync_edges.get(&key) {
            for c in callees {
                if !seen.contains(c) {
                    work.push(c.clone());
                }
            }
        }
    }
    seen
}

/// The diagnostic for one recursive cycle. `from -> to` is the back edge
/// that closed it (and supplies the span — the actual call site a reader
/// has to delete or bound); `cycle` is the whole loop, for the message.
fn recursion_rejection(facts: &Facts, from: &str, to: &str, cycle: &[&str]) -> SemaError {
    // Point at the offending call itself. Every edge in `edges` was
    // recorded by `note_call`, which also pushed a `CallOccurrence`
    // carrying that expression's span, so the lookup always succeeds for
    // a real edge; `Span::default()` is the unreachable belt-and-braces.
    let span = facts
        .callers
        .get(to)
        .and_then(|occs| occs.iter().find(|o| o.holder == from))
        .map(|o| o.span)
        .unwrap_or_default();
    let how = if cycle.len() <= 2 {
        format!("`{to}` calls itself")
    } else {
        format!("`{}`", cycle.join("` -> `"))
    };
    SemaError::at(
        "sema",
        format!(
            "recursive call: {how}. 04-compiler.md §1 rejects unbounded recursion in the call \
             graph — this machine has no stack guard (per-core stacks are packed contiguously in \
             high DRAM, so an overrun silently corrupts the next core's frames rather than \
             faulting), so every call depth must be statically bounded. Rewrite the cycle as a \
             `@budget(bound=N)` loop"
        ),
        span,
    )
}

/// Every declared actor instance's own mailbox capacity, folded to the
/// minimum per actor type — or the reason no capacity is knowable at
/// all. `Err` is a whole-image fact (no `@image` fn, two of them, an
/// `@image` fn that abandons), so it applies to every bare `send`
/// equally.
fn actor_capacities(programs: &BTreeMap<String, &TypedProgram>) -> Result<Capacities, String> {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, p)| p.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    let (module, fn_name) = match candidates.len() {
        // Fail closed, exactly as decision 5 requires: a program with no
        // `@image` in its closure knows no mailbox capacity, so it can
        // prove nothing.
        0 => {
            return Err(
                "the build closure declares no `@image` fn, so no mailbox capacity is \
                         known — every mailbox bound this proof needs is declared there \
                         (`img.actor(T, mailbox=N)`)"
                    .to_string(),
            );
        }
        1 => candidates[0],
        // `bin/wrela.rs` reports this properly at `--stage=image`; here
        // it is only ever a reason the proof cannot run.
        _ => {
            return Err(
                "more than one `@image` fn is reachable in the build closure, so no single \
                 image's mailbox capacities are knowable"
                    .to_string(),
            );
        }
    };
    let program = programs[module];
    let graph = crate::eval::interp::eval_image(program, fn_name).map_err(|e| {
        format!(
            "the `@image` fn `{fn_name}` did not evaluate: {}",
            e.message
        )
    })?;
    Ok(capacities_of(&graph))
}

type Capacities = BTreeMap<String, u64>;

fn capacities_of(graph: &ImageGraph) -> Capacities {
    let mut caps: Capacities = BTreeMap::new();
    for decl in &graph.actors {
        let name = types::render_type(&decl.actor_type);
        let mailbox = decl
            .args
            .iter()
            .find(|a| a.label == "mailbox")
            .and_then(|a| value_as_u64(&a.value));
        // A declaration with no readable `mailbox=` bound contributes a
        // capacity of zero: `layout::compute_runtime_tables` rejects the
        // whole build for it anyway, and until it does, "unknown bound"
        // must never read as "big enough".
        let mailbox = mailbox.unwrap_or(0);
        caps.entry(name)
            .and_modify(|c| *c = (*c).min(mailbox))
            .or_insert(mailbox);
    }
    caps
}

/// Reads a declared `mailbox=` value as a plain non-negative integer —
/// the identical widening `layout::value_as_u64` already applies to the
/// same argument (kept as its own copy rather than made `pub(crate)`
/// across the sema/layout boundary: five lines, one caller each, and the
/// two consumers must be free to disagree about nothing, which a shared
/// helper would only hide).
fn value_as_u64(v: &crate::eval::value::Value) -> Option<u64> {
    use crate::eval::value::Value;
    match *v {
        Value::U8(n) => Some(n as u64),
        Value::U16(n) => Some(n as u64),
        Value::U32(n) => Some(n as u64),
        Value::U64(n) => Some(n),
        Value::Usize(n) => Some(n as u64),
        Value::I8(n) if n >= 0 => Some(n as u64),
        Value::I16(n) if n >= 0 => Some(n as u64),
        Value::I32(n) if n >= 0 => Some(n as u64),
        Value::I64(n) if n >= 0 => Some(n as u64),
        Value::Isize(n) if n >= 0 => Some(n as u64),
        _ => None,
    }
}

// --- the verdict for one bare send ----------------------------------------

fn unprovable_reason(
    site: &MessageSite,
    facts: &Facts,
    capacities: &Result<Capacities, String>,
    in_child: &BTreeSet<String>,
    memo: &mut BTreeMap<String, bool>,
) -> Option<String> {
    let capacities = match capacities {
        Ok(c) => c,
        Err(reason) => return Some(reason.clone()),
    };
    let actor = &site.actor;
    let Some(&capacity) = capacities.get(actor) else {
        return Some(format!(
            "the image declares no instance of actor `{actor}`, so its mailbox has no declared \
             capacity"
        ));
    };
    let targeting: Vec<&MessageSite> = facts.sites.iter().filter(|s| &s.actor == actor).collect();
    let count = targeting.len() as u64;
    if capacity < count {
        return Some(format!(
            "actor `{actor}`'s declared mailbox capacity is {capacity}, but this image has \
             {count} static message site(s) targeting it (every `send`/`await` through an \
             `Actor[{actor}]` handle)"
        ));
    }
    for other in targeting {
        if !other.once_locally {
            return Some(format!(
                "a message site targeting `{actor}` (`{}` in `{}`) sits inside a loop or a \
                 closure body, so it can execute more than once per root turn",
                other.method, other.holder
            ));
        }
        if in_child.contains(&other.holder) {
            return Some(format!(
                "a message site targeting `{actor}` (`{}` in `{}`) sits inside a `g.start` \
                 child, which decision 5 excludes",
                other.method, other.holder
            ));
        }
        if !at_most_once(&other.holder, facts, in_child, memo, &mut BTreeSet::new()) {
            return Some(format!(
                "a message site targeting `{actor}` (`{}` in `{}`) is not provably executed at \
                 most once per root turn — `{}` is reachable from more than one static call \
                 site, from a loop, or through a recursive cycle",
                other.method, other.holder, other.holder
            ));
        }
    }
    None
}

/// Is `key`'s own body executed at most once per root turn? See the
/// module doc's counting rule; `stack` catches recursion (direct or
/// mutual) and answers "no" for every node on a cycle.
fn at_most_once(
    key: &str,
    facts: &Facts,
    in_child: &BTreeSet<String>,
    memo: &mut BTreeMap<String, bool>,
    stack: &mut BTreeSet<String>,
) -> bool {
    if in_child.contains(key) {
        return false;
    }
    if let Some(v) = memo.get(key) {
        return *v;
    }
    if stack.contains(key) {
        return false;
    }
    stack.insert(key.to_string());
    let verdict = match facts.callers.get(key) {
        None => true,
        Some(occs) if occs.is_empty() => true,
        Some(occs) if occs.len() == 1 => {
            occs[0].once_locally && at_most_once(&occs[0].holder, facts, in_child, memo, stack)
        }
        Some(_) => false,
    };
    stack.remove(key);
    memo.insert(key.to_string(), verdict);
    verdict
}

/// Every key reachable from a `g.start` callee through ordinary
/// (non-message) call edges, including the callees themselves — a plain
/// worklist closure, no lattice.
fn group_child_closure(facts: &Facts) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut work: Vec<String> = facts.group_children.iter().cloned().collect();
    while let Some(key) = work.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(callees) = facts.edges.get(&key) {
            for c in callees {
                if !seen.contains(c) {
                    work.push(c.clone());
                }
            }
        }
    }
    seen
}

fn rejection(site: &MessageSite, span: Span, reason: String) -> SemaError {
    let mut e = SemaError::at(
        "actor",
        format!(
            "a bare `send` to `{}.{}` is not proven infallible — consume its `Result[unit, \
             CallError[never]]` (bind it or `match` it)",
            site.actor, site.method
        ),
        span,
    );
    e.extra_lines = vec![
        format!("  {reason}"),
        "  02-language.md §9.4: `send` stands as a bare statement only where mailbox analysis \
         proves admission cannot fail"
            .to_string(),
    ];
    e
}

// --- the scan --------------------------------------------------------------

fn collect(programs: &BTreeMap<String, &TypedProgram>) -> Facts {
    let mut facts = Facts::default();
    for program in programs.values() {
        for (name, f) in &program.fns {
            scan_fn(name.clone(), f, &mut facts);
        }
        for (struct_name, s) in &program.structs {
            for (member, f) in &s.methods {
                scan_fn(format!("{struct_name}.{member}"), f, &mut facts);
            }
            for (member, f) in &s.assoc_fns {
                scan_fn(format!("{struct_name}.{member}"), f, &mut facts);
            }
            if let Some(f) = &s.init {
                scan_fn(format!("{struct_name}.init"), f, &mut facts);
            }
        }
        for (key, inst) in &program.instantiations {
            match inst {
                TypedInstantiation::Fn(f) => scan_fn(key.clone(), f, &mut facts),
                TypedInstantiation::Struct(s) => {
                    for (member, f) in &s.methods {
                        scan_fn(format!("{key}.{member}"), f, &mut facts);
                    }
                    for (member, f) in &s.assoc_fns {
                        scan_fn(format!("{key}.{member}"), f, &mut facts);
                    }
                    if let Some(f) = &s.init {
                        scan_fn(format!("{key}.init"), f, &mut facts);
                    }
                }
                TypedInstantiation::Enum => {}
            }
        }
    }
    facts
}

fn scan_fn(key: String, f: &TypedFn, facts: &mut Facts) {
    let mut cx = Cx {
        holder: key,
        once: true,
        facts,
    };
    cx.stmts(&f.body);
}

struct Cx<'a> {
    holder: String,
    /// Is the position currently being scanned outside every loop and
    /// closure body in this holder?
    once: bool,
    facts: &'a mut Facts,
}

impl Cx<'_> {
    /// `ordinary`: not a message edge (goes into `edges`).
    /// `extends_frame`: the callee's body runs on this holder's own stack
    /// (also goes into `sync_edges` — see that field's own doc for the two
    /// `ordinary`-but-not-frame-extending shapes).
    fn note_call(&mut self, key: &CalleeKey, ordinary: bool, extends_frame: bool, span: Span) {
        let spelling = key.spelling();
        self.facts
            .callers
            .entry(spelling.clone())
            .or_default()
            .push(CallOccurrence {
                holder: self.holder.clone(),
                once_locally: self.once,
                span,
            });
        if ordinary {
            self.facts
                .edges
                .entry(self.holder.clone())
                .or_default()
                .insert(spelling.clone());
        }
        if extends_frame {
            debug_assert!(ordinary, "a message edge never extends the caller's frame");
            self.facts
                .sync_edges
                .entry(self.holder.clone())
                .or_default()
                .insert(spelling);
        }
    }

    /// A `send`/`await` whose inner node is an actor-handle method call
    /// (the only two shapes `bodies::check_send`/`check_await` build; an
    /// `await g.join_all()`'s own inner node is an `Intrinsic` instead).
    /// Records the message site and the callee occurrence — a message
    /// *is* that actor method's invocation — but deliberately **not** an
    /// ordinary call edge: a turn started by a message does not run
    /// inside its sender's frame, so it is not "inside a `g.start`
    /// child" merely because its sender was. Returns false when `inner`
    /// is not an actor call, so the caller can fall back to an ordinary
    /// walk.
    fn note_message(&mut self, inner: &TypedExpr, bare: Option<Span>) -> bool {
        let TypedExprKind::Call {
            callee: callee @ CalleeKey::Method(actor, method),
            receiver: Some(recv),
            args,
        } = &inner.kind
        else {
            return false;
        };
        self.facts.sites.push(MessageSite {
            actor: actor.clone(),
            method: method.clone(),
            holder: self.holder.clone(),
            bare,
            once_locally: self.once,
        });
        self.note_call(callee, false, false, inner.span);
        self.expr(recv);
        for a in args.iter().filter_map(|a| a.value.as_ref()) {
            self.expr(a);
        }
        true
    }

    fn in_loop<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.once;
        self.once = false;
        let r = f(self);
        self.once = saved;
        r
    }

    fn stmts(&mut self, stmts: &[TypedStmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, stmt: &TypedStmt) {
        match &stmt.kind {
            TypedStmtKind::Let { value, .. } => self.expr(value),
            TypedStmtKind::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            TypedStmtKind::If {
                cond,
                then_branch,
                elifs,
                else_branch,
            } => {
                self.expr(cond);
                self.stmts(then_branch);
                for elif in elifs {
                    self.expr(&elif.cond);
                    self.stmts(&elif.body);
                }
                if let Some(b) = else_branch {
                    self.stmts(b);
                }
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    self.stmts(&arm.body);
                }
            }
            TypedStmtKind::For { iter, body, .. } => {
                match iter {
                    TypedForIter::Range(from, to, _) => {
                        self.expr(from);
                        self.expr(to);
                    }
                    TypedForIter::Expr(e) => self.expr(e),
                }
                self.in_loop(|cx| cx.stmts(body));
            }
            TypedStmtKind::While { cond, body, .. } => {
                // The condition is re-evaluated on every iteration too.
                self.in_loop(|cx| {
                    cx.expr(cond);
                    cx.stmts(body);
                });
            }
            TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
            TypedStmtKind::Return(value) => {
                if let Some(e) = value {
                    self.expr(e);
                }
            }
            TypedStmtKind::Assert { cond, message } => {
                self.expr(cond);
                if let Some(m) = message {
                    self.expr(m);
                }
            }
            TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                self.expr(cond);
                if let Some(m) = message {
                    self.expr(m);
                }
            }
            // A `defer` body runs once per registration, so it inherits
            // the registering statement's own position exactly (a
            // `defer` registered inside a loop already has `once =
            // false` here).
            TypedStmtKind::Defer(body) => match body {
                TypedDeferBody::Expr(e) => self.expr(e),
                TypedDeferBody::Suite(stmts) => self.stmts(stmts),
            },
            TypedStmtKind::ExprStmt(e) => self.expr(e),
            TypedStmtKind::BareSend { span, expr } => {
                let TypedExprKind::Send(inner) = &expr.kind else {
                    // Unreachable: `bodies::check_send_stmt` only ever
                    // builds this node around a `Send`. Walk it anyway
                    // rather than silently dropping a subtree.
                    self.expr(expr);
                    return;
                };
                if !self.note_message(inner, Some(*span)) {
                    self.expr(inner);
                }
            }
            TypedStmtKind::WithGroup {
                capacity,
                deadline,
                body,
                ..
            } => {
                if let Some(c) = capacity {
                    self.expr(c);
                }
                if let Some(d) = deadline {
                    self.expr(d);
                }
                self.stmts(body);
            }
        }
    }

    fn expr(&mut self, e: &TypedExpr) {
        match &e.kind {
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
            TypedExprKind::FnRef(key) => self.note_call(key, true, false, e.span),
            TypedExprKind::Field(base, _) => self.expr(base),
            TypedExprKind::Index(base, idx) => {
                self.expr(base);
                self.expr(idx);
            }
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                self.note_call(callee, true, true, e.span);
                if let Some(r) = receiver {
                    self.expr(r);
                }
                for a in args.iter().filter_map(|a| a.value.as_ref()) {
                    self.expr(a);
                }
            }
            TypedExprKind::CallValue(callee, args) => {
                self.expr(callee);
                for a in args.iter().filter_map(|a| a.value.as_ref()) {
                    self.expr(a);
                }
            }
            TypedExprKind::ToScalar(inner)
            | TypedExprKind::Neg(inner)
            | TypedExprKind::BitNot(inner)
            | TypedExprKind::Take(inner)
            | TypedExprKind::Not(inner)
            | TypedExprKind::Panic(inner) => self.expr(inner),
            TypedExprKind::Try(inner, conv) => {
                self.expr(inner);
                if let Some(key) = conv {
                    self.note_call(key, true, true, e.span);
                }
            }
            TypedExprKind::Binary(_, l, r) | TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
                self.expr(l);
                self.expr(r);
            }
            TypedExprKind::OpCall(key, l, r) => {
                self.note_call(key, true, true, e.span);
                self.expr(l);
                self.expr(r);
            }
            TypedExprKind::Is(inner, pat) => {
                self.expr(inner);
                self.pattern(pat);
            }
            TypedExprKind::EnumConstruct { args, .. } => {
                for a in args.iter().filter_map(|a| a.value.as_ref()) {
                    self.expr(a);
                }
            }
            // A closure body is folded into its enclosing holder
            // (`eval::legal`'s own decision-4 reading) but never counts
            // as at-most-once: nothing here bounds how often a closure
            // value is applied.
            TypedExprKind::Closure { body, .. } => self.in_loop(|cx| match body {
                TypedClosureBody::Expr(e) => cx.expr(e),
                TypedClosureBody::Suite(stmts) => cx.stmts(stmts),
            }),
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            TypedExprKind::Intrinsic { receiver, args, .. } => {
                if let Some(r) = receiver {
                    self.expr(r);
                }
                for (_, a) in args {
                    self.expr(a);
                }
            }
            // `await h.m(...)` occupies a mailbox slot exactly like a
            // `send` does, so it is counted the same way; `await
            // g.join_all()` is not a message at all and falls through to
            // the ordinary walk.
            TypedExprKind::Await(inner) => {
                if !self.note_message(inner, None) {
                    self.expr(inner);
                }
            }
            // The consumed expression form: still a message site
            // occupying a mailbox slot, just never itself judged.
            TypedExprKind::Send(inner) => {
                if !self.note_message(inner, None) {
                    self.expr(inner);
                }
            }
            TypedExprKind::GroupChild(key) => {
                self.note_call(key, true, false, e.span);
                self.facts.group_children.insert(key.spelling());
            }
        }
    }

    fn pattern(&mut self, p: &TypedPattern) {
        match &p.kind {
            TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
            TypedPatternKind::Literal(e) => self.expr(e),
            TypedPatternKind::Take(inner) => self.pattern(inner),
            TypedPatternKind::Variant { payload, .. } => {
                for p in payload {
                    self.pattern(p);
                }
            }
            TypedPatternKind::Tuple(elems) | TypedPatternKind::Array(elems) => {
                for p in elems {
                    self.pattern(p);
                }
            }
            TypedPatternKind::Or(alts) => {
                for p in alts {
                    self.pattern(p);
                }
            }
        }
    }
}
