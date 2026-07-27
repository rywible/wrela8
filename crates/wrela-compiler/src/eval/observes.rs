//! Loop-discharge **observes** bit (plans/M13.md item N / decision 11).
//!
//! Leaf observation is by placed address against the machine's pending-vector
//! page and park MMIO — never by static name. The bit propagates through the
//! callee graph the same way comptime legality does (fixed-point over
//! `CalleeKey` spellings). A synchronous `for`/`while` without `@budget` is
//! discharged only when every path from the loop head to its back edge
//! passes an observation point (leaf or call to an observing fn).

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::sema::SemaError;
use crate::sema::typed::{
    CalleeKey, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter, TypedInstantiation,
    TypedProgram, TypedStmt, TypedStmtKind, UnboundedSyncLoop,
};
use crate::syntax::ast::Span;

/// Why a fn's observes bit is set — enough for the typed-dump chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservesReason {
    /// Direct read of the pending-vector page (`wrela_machine::pending::BASE`).
    LeafPendingRead,
    /// Direct write of the park MMIO (`wrela_machine::mmio::PARK_MMIO_ADDR`).
    LeafParkWrite,
    /// Calls an observing callee (spelling).
    Calls(String),
}

/// Whole-program observes classification.
#[derive(Debug, Clone, Default)]
pub struct Observes {
    pub reasons: BTreeMap<String, ObservesReason>,
}

impl Observes {
    pub fn observes(&self, key: &str) -> bool {
        self.reasons.contains_key(key)
    }
}

/// Classify every fn/method/instantiation key.
pub fn classify(program: &TypedProgram) -> Observes {
    let static_addrs = static_addr_map(program);
    let nodes = build_nodes(program, &static_addrs);
    let mut reasons: BTreeMap<String, ObservesReason> = BTreeMap::new();
    for (key, info) in &nodes {
        if let Some(reason) = &info.leaf {
            reasons.insert(key.clone(), reason.clone());
        }
    }
    loop {
        let snapshot = reasons.clone();
        let mut changed = false;
        for (key, info) in &nodes {
            if snapshot.contains_key(key) {
                continue;
            }
            for callee in &info.callees {
                if snapshot.contains_key(callee) {
                    reasons.insert(key.clone(), ObservesReason::Calls(callee.clone()));
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    Observes { reasons }
}

/// Stable dump of the observes chain for `--stage=typed` goldens.
pub fn dump(observes: &Observes) -> String {
    if observes.reasons.is_empty() {
        return String::new();
    }
    let mut out = String::from("Observes\n");
    for (key, reason) in &observes.reasons {
        let r = match reason {
            ObservesReason::LeafPendingRead => "leaf=pending-read".to_string(),
            ObservesReason::LeafParkWrite => "leaf=park-write".to_string(),
            ObservesReason::Calls(c) => format!("calls={c}"),
        };
        out.push_str(&format!("  {key} {r}\n"));
    }
    out
}

/// Refuse unbounded sync loops that do not observation-discharge.
pub fn check_loop_discharge(
    program: &TypedProgram,
    observes: &Observes,
    sites: &[UnboundedSyncLoop],
) -> Result<(), SemaError> {
    let static_addrs = static_addr_map(program);
    for site in sites {
        let Some(body) = find_fn_body(program, &site.fn_name) else {
            continue;
        };
        if fn_is_async(program, &site.fn_name) {
            continue;
        }
        if !nth_unbounded_discharges(body, site.ordinal, observes, &static_addrs) {
            return Err(discharge_err(site.span));
        }
    }
    // Catch sync fns whose unbounded loops were not recorded (should not
    // happen for module-level bodies; fail closed).
    for (name, f) in &program.fns {
        if f.is_async {
            continue;
        }
        check_all_unbounded(&f.body, name, sites, observes, &static_addrs)?;
    }
    for (struct_name, s) in &program.structs {
        for (member, f) in s.methods.iter().chain(s.assoc_fns.iter()) {
            if f.is_async {
                continue;
            }
            let key = format!("{struct_name}.{member}");
            check_all_unbounded(&f.body, &key, sites, observes, &static_addrs)?;
        }
        if let Some(f) = &s.init {
            if !f.is_async {
                let key = format!("{struct_name}.init");
                check_all_unbounded(&f.body, &key, sites, observes, &static_addrs)?;
            }
        }
    }
    Ok(())
}

fn check_all_unbounded(
    body: &[TypedStmt],
    fn_name: &str,
    sites: &[UnboundedSyncLoop],
    observes: &Observes,
    static_addrs: &BTreeMap<String, u64>,
) -> Result<(), SemaError> {
    let mut ord = 0usize;
    let mut err_span = None;
    walk_unbounded(body, &mut |loop_body| {
        if !loop_body_discharges(loop_body, observes, static_addrs) {
            err_span = Some(
                sites
                    .iter()
                    .find(|s| s.fn_name == fn_name && s.ordinal == ord)
                    .map(|s| s.span)
                    .unwrap_or_default(),
            );
            return false;
        }
        ord += 1;
        true
    });
    if let Some(span) = err_span {
        return Err(discharge_err(span));
    }
    Ok(())
}

fn discharge_err(span: Span) -> SemaError {
    SemaError::at(
        "sema",
        "synchronous `for`/`while` requires a preceding `@budget(bound=N)` with \
         comptime-known integer N ≥ 1, or every path from the loop head to its \
         back edge must pass a vector-observation point (02-language.md §8.1)"
            .to_string(),
        span,
    )
}

fn nth_unbounded_discharges(
    body: &[TypedStmt],
    ordinal: usize,
    observes: &Observes,
    static_addrs: &BTreeMap<String, u64>,
) -> bool {
    let mut ord = 0usize;
    let mut ok = true;
    walk_unbounded(body, &mut |loop_body| {
        if ord == ordinal {
            ok = loop_body_discharges(loop_body, observes, static_addrs);
            return false;
        }
        ord += 1;
        true
    });
    ok
}

fn walk_unbounded(stmts: &[TypedStmt], visit: &mut dyn FnMut(&[TypedStmt]) -> bool) {
    for s in stmts {
        match &s.kind {
            TypedStmtKind::While {
                body, budget: None, ..
            }
            | TypedStmtKind::For {
                body, budget: None, ..
            } => {
                if !visit(body) {
                    return;
                }
                walk_unbounded(body, visit);
            }
            TypedStmtKind::While {
                body,
                budget: Some(_),
                ..
            }
            | TypedStmtKind::For {
                body,
                budget: Some(_),
                ..
            } => walk_unbounded(body, visit),
            TypedStmtKind::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                walk_unbounded(then_branch, visit);
                for e in elifs {
                    walk_unbounded(&e.body, visit);
                }
                if let Some(b) = else_branch {
                    walk_unbounded(b, visit);
                }
            }
            TypedStmtKind::Match { arms, .. } => {
                for a in arms {
                    walk_unbounded(&a.body, visit);
                }
            }
            TypedStmtKind::WithGroup { body, .. } => walk_unbounded(body, visit),
            TypedStmtKind::Defer(TypedDeferBody::Suite(b)) => walk_unbounded(b, visit),
            _ => {}
        }
    }
}

/// Every path from body entry to a back edge (`Continue` or fall-through)
/// must have observed. `Break`/`Return` exit the loop and need not.
fn loop_body_discharges(
    body: &[TypedStmt],
    observes: &Observes,
    static_addrs: &BTreeMap<String, u64>,
) -> bool {
    match walk_paths(body, false, observes, static_addrs) {
        PathEnd::BackEdge { observed } => observed,
        PathEnd::ExitLoop => true,
        PathEnd::FallThrough { observed } => observed,
        PathEnd::Fail => false,
    }
}

#[derive(Clone, Copy)]
enum PathEnd {
    FallThrough { observed: bool },
    BackEdge { observed: bool },
    ExitLoop,
    Fail,
}

fn walk_paths(
    stmts: &[TypedStmt],
    mut observed: bool,
    observes: &Observes,
    static_addrs: &BTreeMap<String, u64>,
) -> PathEnd {
    for s in stmts {
        match &s.kind {
            TypedStmtKind::Break | TypedStmtKind::Return(_) => return PathEnd::ExitLoop,
            TypedStmtKind::Continue => return PathEnd::BackEdge { observed },
            TypedStmtKind::Pass => {}
            TypedStmtKind::Assign { target, value } => {
                if expr_observes(value, observes, static_addrs)
                    || target_is_park_write(target, static_addrs)
                {
                    observed = true;
                }
            }
            TypedStmtKind::Let { value, .. } | TypedStmtKind::ExprStmt(value) => {
                if expr_observes(value, observes, static_addrs) {
                    observed = true;
                }
            }
            TypedStmtKind::Assert { cond, message } => {
                if expr_observes(cond, observes, static_addrs)
                    || message
                        .as_ref()
                        .is_some_and(|m| expr_observes(m, observes, static_addrs))
                {
                    observed = true;
                }
            }
            TypedStmtKind::BareSend { expr, .. } => {
                if expr_observes(expr, observes, static_addrs) {
                    observed = true;
                }
            }
            TypedStmtKind::If {
                cond,
                then_branch,
                elifs,
                else_branch,
            } => {
                if expr_observes(cond, observes, static_addrs) {
                    observed = true;
                }
                let mut arms: Vec<&[TypedStmt]> = vec![then_branch.as_slice()];
                for e in elifs {
                    arms.push(e.body.as_slice());
                }
                let empty: &[TypedStmt] = &[];
                match else_branch {
                    Some(b) => arms.push(b.as_slice()),
                    None => arms.push(empty),
                }
                match join_arms(&arms, observed, observes, static_addrs) {
                    PathEnd::Fail => return PathEnd::Fail,
                    PathEnd::ExitLoop => return PathEnd::ExitLoop,
                    PathEnd::BackEdge { observed: o } => {
                        if !o {
                            return PathEnd::Fail;
                        }
                        // A Continue inside an arm already required observed;
                        // remaining stmts are still reachable from fall-through
                        // arms only — handled by FallThrough join below.
                        // If ANY arm continued, we already checked it; if ALL
                        // non-exit arms continued, we're done.
                        return PathEnd::BackEdge { observed: true };
                    }
                    PathEnd::FallThrough { observed: o } => observed = o,
                }
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                if expr_observes(scrutinee, observes, static_addrs) {
                    observed = true;
                }
                let arm_bodies: Vec<&[TypedStmt]> =
                    arms.iter().map(|a| a.body.as_slice()).collect();
                match join_arms(&arm_bodies, observed, observes, static_addrs) {
                    PathEnd::Fail => return PathEnd::Fail,
                    PathEnd::ExitLoop => return PathEnd::ExitLoop,
                    PathEnd::BackEdge { observed: o } => {
                        if !o {
                            return PathEnd::Fail;
                        }
                        return PathEnd::BackEdge { observed: true };
                    }
                    PathEnd::FallThrough { observed: o } => observed = o,
                }
            }
            TypedStmtKind::While {
                body, budget: None, ..
            }
            | TypedStmtKind::For {
                body, budget: None, ..
            } => {
                // Nested unbounded: must discharge on its own; zero-trip
                // means it does not observe for the outer path.
                if !loop_body_discharges(body, observes, static_addrs) {
                    return PathEnd::Fail;
                }
            }
            TypedStmtKind::While {
                cond,
                budget: Some(_),
                ..
            } => {
                if expr_observes(cond, observes, static_addrs) {
                    observed = true;
                }
            }
            TypedStmtKind::For {
                iter,
                budget: Some(_),
                ..
            } => match iter {
                TypedForIter::Range(a, b, _) => {
                    if expr_observes(a, observes, static_addrs)
                        || expr_observes(b, observes, static_addrs)
                    {
                        observed = true;
                    }
                }
                TypedForIter::Expr(e) => {
                    if expr_observes(e, observes, static_addrs) {
                        observed = true;
                    }
                }
            },
            TypedStmtKind::WithGroup {
                capacity,
                deadline,
                body,
                ..
            } => {
                if capacity
                    .as_ref()
                    .is_some_and(|e| expr_observes(e, observes, static_addrs))
                    || deadline
                        .as_ref()
                        .is_some_and(|e| expr_observes(e, observes, static_addrs))
                {
                    observed = true;
                }
                match walk_paths(body, observed, observes, static_addrs) {
                    PathEnd::Fail => return PathEnd::Fail,
                    PathEnd::ExitLoop => return PathEnd::ExitLoop,
                    PathEnd::BackEdge { observed: o } => {
                        if !o {
                            return PathEnd::Fail;
                        }
                        return PathEnd::BackEdge { observed: true };
                    }
                    PathEnd::FallThrough { observed: o } => observed = o,
                }
            }
            TypedStmtKind::Defer(TypedDeferBody::Expr(e)) => {
                if expr_observes(e, observes, static_addrs) {
                    observed = true;
                }
            }
            TypedStmtKind::Defer(TypedDeferBody::Suite(_))
            | TypedStmtKind::ComptimeAssert { .. } => {}
        }
    }
    PathEnd::FallThrough { observed }
}

fn join_arms(
    arms: &[&[TypedStmt]],
    observed: bool,
    observes: &Observes,
    static_addrs: &BTreeMap<String, u64>,
) -> PathEnd {
    let mut fall_obs = Vec::new();
    let mut saw_continue = false;
    let mut saw_fall = false;
    let mut all_exit = true;
    for arm in arms {
        match walk_paths(arm, observed, observes, static_addrs) {
            PathEnd::Fail => return PathEnd::Fail,
            PathEnd::ExitLoop => {}
            PathEnd::BackEdge { observed: o } => {
                all_exit = false;
                saw_continue = true;
                if !o {
                    return PathEnd::Fail;
                }
            }
            PathEnd::FallThrough { observed: o } => {
                all_exit = false;
                saw_fall = true;
                fall_obs.push(o);
            }
        }
    }
    if all_exit {
        return PathEnd::ExitLoop;
    }
    if saw_continue && !saw_fall {
        return PathEnd::BackEdge { observed: true };
    }
    if saw_continue && saw_fall {
        // Some arms continue (already checked), some fall through —
        // join fall-through observation for stmts after the if/match.
        let o = fall_obs.into_iter().all(|x| x);
        return PathEnd::FallThrough { observed: o };
    }
    let o = fall_obs.into_iter().all(|x| x);
    PathEnd::FallThrough { observed: o }
}

fn expr_observes(
    expr: &TypedExpr,
    observes: &Observes,
    static_addrs: &BTreeMap<String, u64>,
) -> bool {
    if expr_is_pending_read(expr, static_addrs) {
        return true;
    }
    match &expr.kind {
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            if observes.observes(&callee.spelling()) {
                return true;
            }
            receiver
                .as_ref()
                .is_some_and(|r| expr_observes(r, observes, static_addrs))
                || args
                    .iter()
                    .flatten()
                    .any(|a| expr_observes(a, observes, static_addrs))
        }
        TypedExprKind::CallValue(f, args) => {
            expr_observes(f, observes, static_addrs)
                || args
                    .iter()
                    .any(|a| expr_observes(a, observes, static_addrs))
        }
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Panic(base) => expr_observes(base, observes, static_addrs),
        TypedExprKind::Try(inner, _) | TypedExprKind::Is(inner, _) => {
            expr_observes(inner, observes, static_addrs)
        }
        TypedExprKind::Index(base, idx) => {
            expr_observes(base, observes, static_addrs)
                || expr_observes(idx, observes, static_addrs)
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            expr_observes(l, observes, static_addrs) || expr_observes(r, observes, static_addrs)
        }
        TypedExprKind::EnumConstruct { args, .. }
        | TypedExprKind::Tuple(args)
        | TypedExprKind::List(args) => args
            .iter()
            .any(|a| expr_observes(a, observes, static_addrs)),
        TypedExprKind::StructLiteral { fields, .. } => fields
            .iter()
            .any(|(_, e)| expr_observes(e, observes, static_addrs)),
        TypedExprKind::Intrinsic { args, .. } => args
            .iter()
            .any(|(_, e)| expr_observes(e, observes, static_addrs)),
        TypedExprKind::Closure { body, .. } => match body {
            crate::sema::typed::TypedClosureBody::Expr(e) => {
                expr_observes(e, observes, static_addrs)
            }
            crate::sema::typed::TypedClosureBody::Suite(_) => false,
        },
        _ => false,
    }
}

// --- graph ---------------------------------------------------------------

struct NodeInfo {
    callees: BTreeSet<String>,
    leaf: Option<ObservesReason>,
}

fn build_nodes(
    program: &TypedProgram,
    static_addrs: &BTreeMap<String, u64>,
) -> BTreeMap<String, NodeInfo> {
    let mut nodes = BTreeMap::new();
    for (name, f) in &program.fns {
        insert_fn(&mut nodes, name.clone(), f, static_addrs);
    }
    for (name, f) in &program.imported.fns {
        if !nodes.contains_key(name) {
            insert_fn(&mut nodes, name.clone(), f, static_addrs);
        }
    }
    for (struct_name, s) in &program.structs {
        for (member, f) in &s.methods {
            insert_fn(
                &mut nodes,
                format!("{struct_name}.{member}"),
                f,
                static_addrs,
            );
        }
        for (member, f) in &s.assoc_fns {
            insert_fn(
                &mut nodes,
                format!("{struct_name}.{member}"),
                f,
                static_addrs,
            );
        }
        if let Some(f) = &s.init {
            insert_fn(&mut nodes, format!("{struct_name}.init"), f, static_addrs);
        }
    }
    for (key, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => insert_fn(&mut nodes, key.clone(), f, static_addrs),
            TypedInstantiation::Struct(s) => {
                for (member, f) in &s.methods {
                    insert_fn(&mut nodes, format!("{key}.{member}"), f, static_addrs);
                }
                for (member, f) in &s.assoc_fns {
                    insert_fn(&mut nodes, format!("{key}.{member}"), f, static_addrs);
                }
                if let Some(f) = &s.init {
                    insert_fn(&mut nodes, format!("{key}.init"), f, static_addrs);
                }
            }
            TypedInstantiation::Enum => {}
        }
    }
    nodes
}

fn static_addr_map(program: &TypedProgram) -> BTreeMap<String, u64> {
    program
        .statics
        .iter()
        .map(|(name, s)| (name.clone(), s.addr))
        .collect()
}

fn insert_fn(
    nodes: &mut BTreeMap<String, NodeInfo>,
    key: String,
    f: &TypedFn,
    static_addrs: &BTreeMap<String, u64>,
) {
    let mut callees = BTreeSet::new();
    let mut leaf = None;
    scan_stmts(&f.body, static_addrs, &mut callees, &mut leaf);
    nodes.insert(key, NodeInfo { callees, leaf });
}

fn scan_stmts(
    stmts: &[TypedStmt],
    static_addrs: &BTreeMap<String, u64>,
    callees: &mut BTreeSet<String>,
    leaf: &mut Option<ObservesReason>,
) {
    for s in stmts {
        match &s.kind {
            TypedStmtKind::Assign { target, value } => {
                if leaf.is_none() && target_is_park_write(target, static_addrs) {
                    *leaf = Some(ObservesReason::LeafParkWrite);
                }
                scan_expr(value, static_addrs, callees, leaf);
                scan_expr(target, static_addrs, callees, leaf);
            }
            TypedStmtKind::Let { value, .. } | TypedStmtKind::ExprStmt(value) => {
                scan_expr(value, static_addrs, callees, leaf);
            }
            TypedStmtKind::If {
                cond,
                then_branch,
                elifs,
                else_branch,
            } => {
                scan_expr(cond, static_addrs, callees, leaf);
                scan_stmts(then_branch, static_addrs, callees, leaf);
                for e in elifs {
                    scan_expr(&e.cond, static_addrs, callees, leaf);
                    scan_stmts(&e.body, static_addrs, callees, leaf);
                }
                if let Some(b) = else_branch {
                    scan_stmts(b, static_addrs, callees, leaf);
                }
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                scan_expr(scrutinee, static_addrs, callees, leaf);
                for a in arms {
                    scan_stmts(&a.body, static_addrs, callees, leaf);
                }
            }
            TypedStmtKind::While { cond, body, .. } => {
                scan_expr(cond, static_addrs, callees, leaf);
                scan_stmts(body, static_addrs, callees, leaf);
            }
            TypedStmtKind::For { iter, body, .. } => {
                match iter {
                    TypedForIter::Range(a, b, _) => {
                        scan_expr(a, static_addrs, callees, leaf);
                        scan_expr(b, static_addrs, callees, leaf);
                    }
                    TypedForIter::Expr(e) => scan_expr(e, static_addrs, callees, leaf),
                }
                scan_stmts(body, static_addrs, callees, leaf);
            }
            TypedStmtKind::Return(Some(e)) => scan_expr(e, static_addrs, callees, leaf),
            TypedStmtKind::Assert { cond, message } => {
                scan_expr(cond, static_addrs, callees, leaf);
                if let Some(m) = message {
                    scan_expr(m, static_addrs, callees, leaf);
                }
            }
            TypedStmtKind::BareSend { expr, .. } => scan_expr(expr, static_addrs, callees, leaf),
            TypedStmtKind::WithGroup {
                capacity,
                deadline,
                body,
                ..
            } => {
                if let Some(e) = capacity {
                    scan_expr(e, static_addrs, callees, leaf);
                }
                if let Some(e) = deadline {
                    scan_expr(e, static_addrs, callees, leaf);
                }
                scan_stmts(body, static_addrs, callees, leaf);
            }
            TypedStmtKind::Defer(TypedDeferBody::Expr(e)) => {
                scan_expr(e, static_addrs, callees, leaf);
            }
            TypedStmtKind::Defer(TypedDeferBody::Suite(b)) => {
                scan_stmts(b, static_addrs, callees, leaf);
            }
            TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                scan_expr(cond, static_addrs, callees, leaf);
                if let Some(m) = message {
                    scan_expr(m, static_addrs, callees, leaf);
                }
            }
            _ => {}
        }
    }
}

fn scan_expr(
    expr: &TypedExpr,
    static_addrs: &BTreeMap<String, u64>,
    callees: &mut BTreeSet<String>,
    leaf: &mut Option<ObservesReason>,
) {
    if leaf.is_none() && expr_is_pending_read(expr, static_addrs) {
        *leaf = Some(ObservesReason::LeafPendingRead);
    }
    match &expr.kind {
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            callees.insert(callee_spelling(callee));
            if let Some(r) = receiver {
                scan_expr(r, static_addrs, callees, leaf);
            }
            for a in args.iter().flatten() {
                scan_expr(a, static_addrs, callees, leaf);
            }
        }
        TypedExprKind::CallValue(f, args) => {
            scan_expr(f, static_addrs, callees, leaf);
            for a in args {
                scan_expr(a, static_addrs, callees, leaf);
            }
        }
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Panic(base) => scan_expr(base, static_addrs, callees, leaf),
        TypedExprKind::Try(inner, _) | TypedExprKind::Is(inner, _) => {
            scan_expr(inner, static_addrs, callees, leaf);
        }
        TypedExprKind::Index(base, idx) => {
            scan_expr(base, static_addrs, callees, leaf);
            scan_expr(idx, static_addrs, callees, leaf);
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            scan_expr(l, static_addrs, callees, leaf);
            scan_expr(r, static_addrs, callees, leaf);
        }
        TypedExprKind::EnumConstruct { args, .. }
        | TypedExprKind::Tuple(args)
        | TypedExprKind::List(args) => {
            for a in args {
                scan_expr(a, static_addrs, callees, leaf);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, e) in fields {
                scan_expr(e, static_addrs, callees, leaf);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            crate::sema::typed::TypedClosureBody::Expr(e) => {
                scan_expr(e, static_addrs, callees, leaf);
            }
            crate::sema::typed::TypedClosureBody::Suite(stmts) => {
                scan_stmts(stmts, static_addrs, callees, leaf);
            }
        },
        TypedExprKind::Intrinsic { args, .. } => {
            for (_, a) in args {
                scan_expr(a, static_addrs, callees, leaf);
            }
        }
        TypedExprKind::FnRef(key) => {
            callees.insert(callee_spelling(key));
        }
        _ => {}
    }
}

fn callee_spelling(key: &CalleeKey) -> String {
    key.spelling()
}

fn expr_is_pending_read(expr: &TypedExpr, static_addrs: &BTreeMap<String, u64>) -> bool {
    let Some(addr) = expr_static_base(expr, static_addrs) else {
        return false;
    };
    let base = wrela_machine::pending::BASE;
    let end = base + wrela_machine::pending::SIZE;
    addr >= base && addr < end
}

fn target_is_park_write(target: &TypedExpr, static_addrs: &BTreeMap<String, u64>) -> bool {
    let Some(addr) = expr_static_base(target, static_addrs) else {
        return false;
    };
    addr == wrela_machine::mmio::PARK_MMIO_ADDR
}

fn expr_static_base(expr: &TypedExpr, static_addrs: &BTreeMap<String, u64>) -> Option<u64> {
    match &expr.kind {
        TypedExprKind::Static(name) => static_addrs.get(name).copied(),
        TypedExprKind::Field(base, _)
        | TypedExprKind::Index(base, _)
        | TypedExprKind::Take(base) => expr_static_base(base, static_addrs),
        _ => None,
    }
}

fn find_fn_body<'a>(program: &'a TypedProgram, key: &str) -> Option<&'a [TypedStmt]> {
    if let Some(f) = program.fns.get(key) {
        return Some(&f.body);
    }
    let (struct_name, member) = key.split_once('.')?;
    let s = program.structs.get(struct_name)?;
    if let Some(f) = s.methods.get(member) {
        return Some(&f.body);
    }
    if let Some(f) = s.assoc_fns.get(member) {
        return Some(&f.body);
    }
    if member == "init" {
        return s.init.as_ref().map(|f| f.body.as_slice());
    }
    None
}

fn fn_is_async(program: &TypedProgram, key: &str) -> bool {
    if let Some(f) = program.fns.get(key) {
        return f.is_async;
    }
    let Some((struct_name, member)) = key.split_once('.') else {
        return false;
    };
    let Some(s) = program.structs.get(struct_name) else {
        return false;
    };
    if let Some(f) = s.methods.get(member) {
        return f.is_async;
    }
    if let Some(f) = s.assoc_fns.get(member) {
        return f.is_async;
    }
    if member == "init" {
        return s.init.as_ref().is_some_and(|f| f.is_async);
    }
    false
}
