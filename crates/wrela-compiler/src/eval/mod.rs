//! The comptime evaluator (plans/M3.md item B): a tree-walking
//! interpreter over the typed tree (`sema::typed`), the reference
//! implementation of comptime semantics (ROADMAP.md: "the tree-walking
//! evaluator is the reference implementation of the semantics").
//!
//! - `value.rs` — `Value`, decision 5's one plain enum, plus exact
//!   scalar arithmetic (docs/language/02-language.md §6.1).
//! - `interp.rs` — the one obvious recursive walk (decision 4): statement/
//!   expression evaluation, calls, `defer`, `?`, quotas.
//! - `quota.rs` — decision 6's fixed step/memory/call-depth budgets.
//!
//! `legal.rs` (whole-graph comptime-legality inference, plans/M3.md item
//! C) lands separately, in parallel with this item — not part of item
//! B's own deliverable.
//!
//! Integration surface (the only user-visible one item B adds): const
//! initializers and const-generic arguments are evaluated with the real
//! evaluator here, replacing M2-H's literal-only subset
//! (`sema::generics::eval_const_expr`, now backed by `eval_standalone`
//! below instead of its own hand-rolled literal/const-name/fieldless-
//! variant match). This module does not gate on comptime legality
//! (plans/M3.md item C, parallel) — an illegal construct (`await`/
//! `send`/`with`/pool operations) cannot reach the typed tree in the
//! first place (sema already fails those closed before producing one),
//! so the evaluator's own fail-closed behavior on anything it does not
//! implement is the only guard item B needs.

pub mod image;
pub mod image_checks;
pub mod interp;
pub mod legal;
pub mod quota;
pub mod value;

use crate::sema::SemaError;
use crate::sema::typed::{
    TestKind, TypedClosureBody, TypedDeferBody, TypedExpr, TypedFn, TypedForIter,
    TypedInstantiation, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::Type;
use crate::syntax::ast::Span;

pub use interp::EvalError;
pub use value::Value;

/// Renders one `EvalError` as a `sema::SemaError` (category `comptime`):
/// typed nodes carry no spans, so — like `generics.rs`'s own multi-line
/// requirement-chain diagnostic — this reuses `omit_location`/
/// `extra_lines` rather than inventing a fake `L:C`. The primary line
/// names the operation that abandoned; each `extra_lines` entry is one
/// live call-stack frame, outermost (the const/context that kicked off
/// evaluation) first.
pub fn to_sema_error(e: EvalError) -> SemaError {
    let extra_lines = e
        .stack
        .iter()
        .map(|frame| format!("  while evaluating `{frame}`"))
        .collect();
    SemaError {
        category: "comptime",
        message: e.message,
        line: 0,
        col: 0,
        extra_lines,
        omit_location: true,
        missing_method: None,
    }
}

/// The whole comptime-evaluation tail of `sema::mod::check_typed`
/// (plans/M3.md item D): computes the program-wide legality
/// classification exactly once (`legal::classify` — item C×D's own
/// wiring point) and threads it through both `check_consts` and
/// `check_comptime_asserts` below, rather than each recomputing the same
/// whole-graph callee scan separately. Runs asserts first (decision 8
/// frames `comptime assert` as proving preconditions; nothing else here
/// depends on the order, but a fixed one keeps failures deterministic
/// when a program has more than one kind of comptime error).
pub fn check_comptime(program: &TypedProgram) -> Result<(), SemaError> {
    let legality = legal::classify(program);
    check_comptime_asserts(program, &legality)?;
    check_consts(program, &legality)?;
    check_test_legality(program, &legality)?;
    check_image_legality(program, &legality)
}

/// plans/M4.md item B, post-review fix: the one reachable `@image` fn
/// must itself be a comptime-legal closure exactly like a `@test` is
/// (`check_test_legality` above) — `legal::classify` exempts that fn's
/// own node from being marked illegal by an intrinsic it *directly*
/// writes (decision 5's whole point), but that exemption alone never
/// required the fn's own verdict to be checked at all, so a transitively
/// illegal callee (an ordinary helper fn that itself calls a builder
/// intrinsic on a `mut img: Image` parameter, say) evaluated anyway —
/// the disclosed "helper fns are not exempted" boundary was true of the
/// classification but not of anything that consulted it. Fixed here: the
/// `@image` fn's own direct callees (`legal::direct_callees_of_body` —
/// intrinsics contribute no callee key at all, only ordinary `Call`/
/// `FnRef`/`OpCall`/`Try` targets do, so this is exactly "everything the
/// `@image` fn calls that lives in a graph node") are checked before
/// `require_legal` ever runs on the fn's own (whole-transitive-closure)
/// verdict — reusing the identical chain diagnostic every other context
/// already gets. A callee this module's own `Legality` has no node for
/// at all (an imported/spliced fn or struct method: `sema::bodies::check`'s
/// own per-item loop only ever populates `TypedProgram::fns`/`structs`
/// from `module.items`, never from an import binding, so a cross-module
/// callee is invisible to `legal::classify` here) is **not** silently
/// treated as legal (`Legality::verdict`'s own documented default for an
/// unknown key, which is a `@image`-day-1 statement of "nothing illegal
/// is representable yet", not a promise this new intrinsic-bearing case
/// should reuse) — it fails closed instead, naming the callee and the
/// real reason: this module's own build cannot yet prove or disprove a
/// cross-module callee's comptime legality. A no-op for every existing
/// golden (each `@image` fn's own direct callees are either intrinsics,
/// which contribute no callee key, or ordinary same-module helper fns,
/// which are always classifiable).
fn check_image_legality(
    program: &TypedProgram,
    legality: &legal::Legality,
) -> Result<(), SemaError> {
    let Some(image_fn) = &program.image_fn else {
        return Ok(());
    };
    let Some(f) = program.fns.get(image_fn) else {
        return Ok(()); // internal invariant: `image_fn` always names a checked fn.
    };
    for callee in legal::direct_callees_of_body(&f.body) {
        if !legality.verdicts.contains_key(&callee) {
            return Err(SemaError {
                category: "unimplemented",
                message: format!(
                    "`@image` fn `{image_fn}` calls `{callee}`, declared in another module; \
                     checking a cross-module callee's comptime legality from an `@image` fn is \
                     not supported yet"
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
    }
    legal::require_legal(
        legality,
        image_fn,
        &format!("@image fn {image_fn}"),
        Span::default(),
    )
}

/// plans/M4.md item B: every comptime-run `@test` (`TestKind::Comptime`/
/// `TestKind::Exhaustive` — `TestKind::Runtime` is deliberately exempt,
/// 02-language.md §12.2's own "generated image test" path, M5) must
/// itself be comptime-legal, checked here (unconditionally, as part of
/// ordinary `check_typed`/`check`) rather than only lazily inside
/// `wrela test`'s own report (`run_tests`, below) — `comptime.legality.inferred`'s
/// own flip needs "an intrinsic in a `@test` fn" to be an honest build
/// error at `--stage=check`/`--stage=typed`, not merely a `FAILED` test
/// line. A no-op for every program before this item (no `@test` could
/// ever be `Illegal` — `eval::legal`'s own module doc), so this adds no
/// new failure to any existing golden.
fn check_test_legality(
    program: &TypedProgram,
    legality: &legal::Legality,
) -> Result<(), SemaError> {
    for t in &program.tests {
        if matches!(t.kind, TestKind::Comptime | TestKind::Exhaustive) {
            legal::require_legal(legality, &t.name, "@test", Span::default())?;
        }
    }
    Ok(())
}

/// Evaluates every module-level `const`'s own initializer with the real
/// evaluator (the integration surface, plans/M3.md item B) — called
/// once, after the typed program is fully assembled (`sema::mod::check_typed`,
/// past `generics::check`, so a const initializer that calls into a
/// generic-fn instantiation can resolve it); fail-fast, `BTreeMap`
/// (name) order, matching every other pass's own diagnostic ordering
/// convention (CLAUDE.md: deterministic, first error wins).
///
/// plans/M3.md item D's own legality wiring: every direct callee a
/// const's own initializer reaches (`legal::scan_standalone`) must be
/// comptime-legal per the already-computed whole-program `legality`
/// (context `"const <name>"`, matching `eval::legal`'s own doc-comment
/// example verbatim) — checked before evaluating, so an illegal closure
/// is diagnosed as such rather than however evaluating it might
/// otherwise fail. `Span::default()` is this diagnostic's own location
/// today: nothing representable could be illegal via a *callee* before
/// plans/M4.md item B (see `eval::legal`'s module doc) — a real span is
/// not worth threading through `TypedConst` for that half of this
/// check. plans/M4.md item B adds the second half: a builder intrinsic
/// used *directly* in a const initializer carries no callee key at all
/// (`legal::StandaloneScan::illegal`), so it is checked and diagnosed
/// separately, right here.
fn check_consts(program: &TypedProgram, legality: &legal::Legality) -> Result<(), SemaError> {
    for (name, c) in &program.consts {
        let scan = legal::scan_standalone(&c.value);
        for callee in &scan.callees {
            legal::require_legal(legality, callee, &format!("const {name}"), Span::default())?;
        }
        if let Some(reason) = &scan.illegal {
            return Err(SemaError {
                category: "comptime",
                message: format!(
                    "`const {name}` requires a comptime-legal initializer, but it directly uses {reason}"
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
        interp::eval_const(program, name).map_err(to_sema_error)?;
    }
    Ok(())
}

/// `wrela test`'s own report (plans/M3.md item E, decision 9): every
/// `@test` fn's own verdict, one line apiece, in declaration order
/// (`program.tests`, `sema::typed::TypedProgram::tests`'s own doc
/// comment — source order, never `BTreeMap` order), followed by one
/// pinned summary line. Never fail-fasts across tests (decision 9: "the
/// report is still the complete stable dump" even when some test
/// failed) — every declared test always gets its own line and its own
/// fresh quota, regardless of how any other test came out. Returns the
/// full report text plus whether the caller's own exit code should be
/// nonzero (`true` iff at least one test's line is `FAILED`).
///
/// Line format, pinned by golden coverage (`comptime.tests.build-tier`,
/// `comptime.tests.exhaustive`), chosen once and never varied:
///   `test <name>: ok`
///   `test <name>: ok (<N> cases)`        (exhaustive only)
///   `test <name>: FAILED <first line of the diagnostic>`
///   `test <name>: FAILED [<param>=<value>, ...] <first line>`  (exhaustive counterexample)
///   `<N> passed, <M> failed`
/// A file with no `@test` fns at all still prints the summary line alone
/// (`0 passed, 0 failed`) — the dumbest honest "ran zero tests" report,
/// not a special-cased error.
///
/// Fail-closed per decision 9/10: `@test(runtime)` is a *legal*
/// declaration (02-language.md §12.2 — it just names a different, not-
/// yet-built execution path, a generated image test on the wrela
/// machine runner, M5), so sema accepts its body like any other fn's;
/// this is the one place that refuses to *run* it, printing a FAILED
/// line naming M5 rather than silently skipping it or attempting
/// something M5 alone can build. A comptime-illegal `@test` closure
/// (decision 7's `legal::classify`) gets the identical fail-closed
/// treatment — today unreachable (no illegal operation is representable
/// in the typed tree yet, `eval::legal`'s own module doc), but wired
/// through so the day one becomes representable, its own `@test` fails
/// exactly this way rather than panicking or silently passing.
pub fn run_tests(program: &TypedProgram) -> (String, bool) {
    let legality = legal::classify(program);
    let mut out = String::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    for test in &program.tests {
        let line = match test.kind {
            TestKind::Runtime => {
                failed += 1;
                format!(
                    "test {}: FAILED `@test(runtime)` is not run yet (M5: generated image tests)",
                    test.name
                )
            }
            TestKind::Comptime => {
                match legal::require_legal(&legality, &test.name, "@test", Span::default()) {
                    Err(e) => {
                        failed += 1;
                        format!(
                            "test {}: FAILED {} (M5: illegal-closure tests run as image tests)",
                            test.name, e.message
                        )
                    }
                    Ok(()) => match interp::eval_test(program, &test.name) {
                        Ok(_) => {
                            passed += 1;
                            format!("test {}: ok", test.name)
                        }
                        Err(e) => {
                            failed += 1;
                            let first_line = e.message.lines().next().unwrap_or("");
                            format!("test {}: FAILED {first_line}", test.name)
                        }
                    },
                }
            }
            TestKind::Exhaustive => {
                match legal::require_legal(&legality, &test.name, "@test", Span::default()) {
                    Err(e) => {
                        failed += 1;
                        format!(
                            "test {}: FAILED {} (M5: illegal-closure tests run as image tests)",
                            test.name, e.message
                        )
                    }
                    Ok(()) => match run_exhaustive_test(program, &test.name) {
                        Ok(cases) => {
                            passed += 1;
                            format!("test {}: ok ({cases} cases)", test.name)
                        }
                        Err(line) => {
                            failed += 1;
                            format!("test {}: FAILED {line}", test.name)
                        }
                    },
                }
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!("{passed} passed, {failed} failed\n"));
    (out, failed > 0)
}

/// One `@test(exhaustive)` fn's whole enumeration (02-language.md
/// §12.2): every parameter's finite domain (validated at declaration —
/// `sema::bodies::check_exhaustive_test_params` — so an unenumerable
/// type here is an internal error, fail closed), cartesian product in
/// declaration order with the rightmost parameter varying fastest, one
/// `interp::eval_test_case` per case under its own fresh quota, stopping
/// at the first failing case — the enumeration order is fixed, so the
/// reported counterexample is deterministic. `Ok` carries the case
/// count for the report line; `Err` carries everything after `FAILED `
/// (the `[param=value, ...]` counterexample prefix when a case failed,
/// or the cap/internal-error text when the enumeration never ran).
fn run_exhaustive_test(program: &TypedProgram, name: &str) -> Result<u128, String> {
    let Some(f) = program.fns.get(name) else {
        return Err(format!(
            "internal error: test fn `{name}` not found in the checked program"
        ));
    };
    let mut domains: Vec<Vec<Value>> = Vec::new();
    for p in &f.params {
        let Some(domain) = param_domain(program, &p.ty) else {
            return Err(format!(
                "internal error: parameter `{}` has no enumerable domain",
                p.name
            ));
        };
        domains.push(domain);
    }
    let total: u128 = domains.iter().map(|d| d.len() as u128).product();
    if total > quota::MAX_EXHAUSTIVE_CASES {
        return Err(format!(
            "exhaustive domain has {total} cases, over the {} cap",
            quota::MAX_EXHAUSTIVE_CASES
        ));
    }
    // Odometer over the domains, rightmost fastest.
    let mut indices = vec![0usize; domains.len()];
    for _ in 0..total {
        let args: Vec<Value> = indices
            .iter()
            .zip(domains.iter())
            .map(|(&i, d)| d[i].clone())
            .collect();
        if let Err(e) = interp::eval_test_case(program, name, &args) {
            let first_line = e.message.lines().next().unwrap_or("");
            return Err(format!(
                "[{}] {first_line}",
                f.params
                    .iter()
                    .zip(args.iter())
                    .map(|(p, v)| format!("{}={}", p.name, render_case_value(program, &p.ty, v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for slot in (0..indices.len()).rev() {
            indices[slot] += 1;
            if indices[slot] < domains[slot].len() {
                break;
            }
            indices[slot] = 0;
        }
    }
    Ok(total)
}

/// The finite domain of one `@test(exhaustive)` parameter type, in its
/// canonical order (`false` before `true`; numeric ascending; enum
/// variants in declaration order — `TypedProgram::enums`). `None` for
/// anything sema should already have rejected.
fn param_domain(program: &TypedProgram, ty: &Type) -> Option<Vec<Value>> {
    match ty {
        Type::Bool => Some(vec![Value::Bool(false), Value::Bool(true)]),
        Type::U8 => Some((0..=u8::MAX).map(Value::U8).collect()),
        Type::I8 => Some((i8::MIN..=i8::MAX).map(Value::I8).collect()),
        // plans/M9.md item A1b: this module's own enums, else the ones it
        // imports — an imported enum is as finite a domain as a local one.
        Type::Named(name, targs) if targs.is_empty() => {
            let en = program
                .enums
                .get(name)
                .or_else(|| program.imported.enums.get(name))?;
            Some(
                (0..en.variants.len())
                    .map(|i| Value::Enum(i, vec![]))
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Renders one enumerated case value for the counterexample line —
/// exactly the shapes `param_domain` can produce, nothing more.
fn render_case_value(program: &TypedProgram, ty: &Type, v: &Value) -> String {
    match (ty, v) {
        (Type::Named(name, _), Value::Enum(idx, _)) => match program
            .enums
            .get(name)
            .or_else(|| program.imported.enums.get(name))
        {
            Some(en) => en
                .variants
                .get(*idx)
                .cloned()
                .unwrap_or_else(|| format!("<variant {idx}>")),
            None => format!("<variant {idx}>"),
        },
        (_, Value::Bool(b)) => b.to_string(),
        (_, Value::U8(n)) => n.to_string(),
        (_, Value::I8(n)) => n.to_string(),
        _ => "<value>".to_string(),
    }
}

/// Evaluates every `comptime assert` statement anywhere in the typed
/// program exactly once (plans/M3.md item D, decision 8: "`comptime
/// assert` evaluates after typing; failure is a build error with the
/// message") — unconditionally, independent of whether anything ever
/// calls the fn/method/instantiation it lives in (`interp::exec_stmt`'s
/// own no-op arm for `TypedStmtKind::ComptimeAssert` explains why that
/// must be a separate, whole-program walk rather than piggybacking on
/// ordinary per-call execution: a `comptime assert` inside a fn nothing
/// currently calls would otherwise never be checked at all, which would
/// make it prove nothing). Walks every fn/method/instantiation body in
/// the same `BTreeMap` order `legal::classify` itself enumerates them in
/// (fns, then structs' methods/assoc-fns/init, then instantiations),
/// depth-first through nested statement blocks in source order — fail-
/// fast, deterministic.
fn check_comptime_asserts(
    program: &TypedProgram,
    legality: &legal::Legality,
) -> Result<(), SemaError> {
    for f in program.fns.values() {
        check_asserts_in_fn(program, legality, f)?;
    }
    for s in program.structs.values() {
        check_asserts_in_struct(program, legality, s)?;
    }
    for e in program.enums.values() {
        check_asserts_in_enum(program, legality, e)?;
    }
    for inst in program.instantiations.values() {
        match inst {
            TypedInstantiation::Fn(f) => check_asserts_in_fn(program, legality, f)?,
            TypedInstantiation::Struct(s) => check_asserts_in_struct(program, legality, s)?,
            TypedInstantiation::Enum => {}
        }
    }
    Ok(())
}

fn check_asserts_in_struct(
    program: &TypedProgram,
    legality: &legal::Legality,
    s: &TypedStruct,
) -> Result<(), SemaError> {
    for f in s.methods.values() {
        check_asserts_in_fn(program, legality, f)?;
    }
    for f in s.assoc_fns.values() {
        check_asserts_in_fn(program, legality, f)?;
    }
    if let Some(f) = &s.init {
        check_asserts_in_fn(program, legality, f)?;
    }
    Ok(())
}

fn check_asserts_in_enum(
    program: &TypedProgram,
    legality: &legal::Legality,
    e: &crate::sema::typed::TypedEnum,
) -> Result<(), SemaError> {
    for f in e.methods.values() {
        check_asserts_in_fn(program, legality, f)?;
    }
    for f in e.assoc_fns.values() {
        check_asserts_in_fn(program, legality, f)?;
    }
    Ok(())
}

fn check_asserts_in_fn(
    program: &TypedProgram,
    legality: &legal::Legality,
    f: &TypedFn,
) -> Result<(), SemaError> {
    let mut sites = Vec::new();
    collect_asserts_stmts(&f.body, &mut sites);
    for site in sites {
        check_one_comptime_assert(program, legality, site)?;
    }
    Ok(())
}

/// One `comptime assert` statement found by the collector below,
/// borrowed straight out of the typed tree (no cloning — the collection
/// and the checking both happen within `check_asserts_in_fn`'s own call,
/// before anything else could need a mutable borrow of `program`).
struct AssertSite<'p> {
    cond: &'p TypedExpr,
    message: Option<&'p TypedExpr>,
    span: Span,
}

fn check_one_comptime_assert(
    program: &TypedProgram,
    legality: &legal::Legality,
    site: AssertSite<'_>,
) -> Result<(), SemaError> {
    let cond_scan = legal::scan_standalone(site.cond);
    for callee in &cond_scan.callees {
        legal::require_legal(legality, callee, "comptime assert", site.span)?;
    }
    if let Some(reason) = &cond_scan.illegal {
        return Err(SemaError::at(
            "comptime",
            format!("`comptime assert` directly uses {reason}, only legal inside an `@image` fn"),
            site.span,
        ));
    }
    if let Some(m) = site.message {
        let msg_scan = legal::scan_standalone(m);
        for callee in &msg_scan.callees {
            legal::require_legal(legality, callee, "comptime assert", site.span)?;
        }
        if let Some(reason) = &msg_scan.illegal {
            return Err(SemaError::at(
                "comptime",
                format!(
                    "`comptime assert` directly uses {reason}, only legal inside an `@image` fn"
                ),
                site.span,
            ));
        }
    }

    let cond_value = interp::eval_standalone(program, site.cond, "comptime assert".to_string())
        .map_err(to_sema_error)?;
    if interp::as_bool(&cond_value) {
        return Ok(());
    }
    let msg = match site.message {
        Some(m) => {
            let mv = interp::eval_standalone(program, m, "comptime assert".to_string())
                .map_err(to_sema_error)?;
            format!(": {}", interp::render_message(&mv))
        }
        None => String::new(),
    };
    Err(SemaError {
        category: "comptime",
        message: format!("comptime assert failed{msg}"),
        line: site.span.line,
        col: site.span.col,
        extra_lines: Vec::new(),
        omit_location: false,
        missing_method: None,
    })
}

// --- the whole-program `comptime assert` collector -------------------------
//
// Mirrors `eval::legal`'s own exhaustive `scan_stmt`/`scan_expr` shape
// (same reason: a new node kind should force a compile error here, not
// silently go unwalked) but collects `AssertSite`s by reference instead
// of callees by key — kept as its own small walk rather than widening
// `legal::BodyScan` with a lifetime parameter every one of its own
// existing call sites would then have to thread through.

fn collect_asserts_stmts<'p>(stmts: &'p [TypedStmt], out: &mut Vec<AssertSite<'p>>) {
    for s in stmts {
        collect_asserts_stmt(s, out);
    }
}

fn collect_asserts_stmt<'p>(stmt: &'p TypedStmt, out: &mut Vec<AssertSite<'p>>) {
    match &stmt.kind {
        TypedStmtKind::ComptimeAssert {
            span,
            cond,
            message,
        } => {
            out.push(AssertSite {
                cond,
                message: message.as_ref(),
                span: *span,
            });
        }
        TypedStmtKind::Let { value, .. } => collect_asserts_expr(value, out),
        TypedStmtKind::Assign { target, value } => {
            collect_asserts_expr(target, out);
            collect_asserts_expr(value, out);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            collect_asserts_expr(cond, out);
            collect_asserts_stmts(then_branch, out);
            for elif in elifs {
                collect_asserts_expr(&elif.cond, out);
                collect_asserts_stmts(&elif.body, out);
            }
            if let Some(b) = else_branch {
                collect_asserts_stmts(b, out);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            collect_asserts_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_asserts_expr(g, out);
                }
                collect_asserts_stmts(&arm.body, out);
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(from, to, _) => {
                    collect_asserts_expr(from, out);
                    collect_asserts_expr(to, out);
                }
                TypedForIter::Expr(e) => collect_asserts_expr(e, out),
            }
            collect_asserts_stmts(body, out);
        }
        TypedStmtKind::While { cond, body } => {
            collect_asserts_expr(cond, out);
            collect_asserts_stmts(body, out);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(value) => {
            if let Some(e) = value {
                collect_asserts_expr(e, out);
            }
        }
        TypedStmtKind::Assert { cond, message } => {
            collect_asserts_expr(cond, out);
            if let Some(m) = message {
                collect_asserts_expr(m, out);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => collect_asserts_expr(e, out),
            TypedDeferBody::Suite(stmts) => collect_asserts_stmts(stmts, out),
        },
        TypedStmtKind::ExprStmt(e) => collect_asserts_expr(e, out),
        // plans/M6.md item G: a bare `send` statement carries an
        // ordinary message call whose arguments could embed a
        // `comptime assert` exactly like any other expression's.
        TypedStmtKind::BareSend { expr, .. } => collect_asserts_expr(expr, out),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                collect_asserts_expr(c, out);
            }
            if let Some(d) = deadline {
                collect_asserts_expr(d, out);
            }
            collect_asserts_stmts(body, out);
        }
    }
}

fn collect_asserts_expr<'p>(e: &'p TypedExpr, out: &mut Vec<AssertSite<'p>>) {
    use crate::sema::typed::TypedExprKind;
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
        | TypedExprKind::FnRef(_) => {}
        TypedExprKind::Field(base, _) => collect_asserts_expr(base, out),
        TypedExprKind::Index(base, idx) => {
            collect_asserts_expr(base, out);
            collect_asserts_expr(idx, out);
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                collect_asserts_expr(r, out);
            }
            for a in args.iter().flatten() {
                collect_asserts_expr(a, out);
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            collect_asserts_expr(callee, out);
            for a in args {
                collect_asserts_expr(a, out);
            }
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Not(inner) => collect_asserts_expr(inner, out),
        TypedExprKind::Try(inner, _) => collect_asserts_expr(inner, out),
        TypedExprKind::Binary(_, l, r) | TypedExprKind::OpCall(_, l, r) => {
            collect_asserts_expr(l, out);
            collect_asserts_expr(r, out);
        }
        TypedExprKind::Is(inner, _) => collect_asserts_expr(inner, out),
        TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            collect_asserts_expr(l, out);
            collect_asserts_expr(r, out);
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                collect_asserts_expr(a, out);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => collect_asserts_expr(e, out),
            TypedClosureBody::Suite(stmts) => collect_asserts_stmts(stmts, out),
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                collect_asserts_expr(i, out);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                collect_asserts_expr(v, out);
            }
        }
        TypedExprKind::Panic(msg) => collect_asserts_expr(msg, out),
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                collect_asserts_expr(r, out);
            }
            for (_, a) in args {
                collect_asserts_expr(a, out);
            }
        }
        TypedExprKind::PoolName(_) => {}
        // Plans/M6.md item A: a `comptime assert` is evaluated
        // unconditionally (decision 8, `check_comptime_asserts`'s own doc
        // comment) independent of whether the fn it lives in is itself
        // comptime-legal — so this collection walk must still find one
        // nested inside an `await`/`send`/group construct, exactly like
        // every other expression shape above.
        TypedExprKind::Await(inner) | TypedExprKind::Send(inner) => {
            collect_asserts_expr(inner, out)
        }
        TypedExprKind::GroupChild(_) => {}
    }
}
