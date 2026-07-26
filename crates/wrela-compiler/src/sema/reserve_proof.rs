//! The `reserve_proven` proof (plans/M7.md item E2, decision 6;
//! 03-hardware.md §4): "`reserve_proven` exists only when whole-image
//! analysis proves every admitted handler a complete unit (three direct
//! descriptors in a 128-deep queue means at most 42 in flight — the
//! compiler computes it)."
//!
//! ## Shape (decision 6)
//!
//! Same analysis shape as `sema::send_proof`: count static sites, require
//! each to be at-most-once per root turn, require the count to fit the
//! declared bound. The bound here is descriptor capacity —
//! `floor(queue_depth / descriptors_per_site)` — not a mailbox size.
//! Same fail-closed floor, same "if the proof is too weak to ever fire,
//! say so" honesty.
//!
//! ## Where this runs, and why
//!
//! At the *end* of `sema::check_typed`/`check_program_typed`, beside
//! `send_proof`, for the same reason: a queue's depth is a declared fact
//! (`VirtQueue[..N]` / `VirtQueue.configure`'s `depth=`), and a program
//! with no image-configured queue can still be judged from the
//! receiver's own type (the Bound the checker resolved to a literal).
//! Returns immediately unless the closure contains a
//! `VirtQueue.reserve_proven` intrinsic.
//!
//! Runtime backpressure (03 §4's generated proxy) is **not** this pass
//! (plans/M7.md decision 6 / decision 15: item G).

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::SemaError;
use crate::sema::typed::{
    CalleeKey, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{self, TypeArg};
use crate::syntax::ast::{Expr, Span};
use crate::virtqueue;

// --- collected facts -------------------------------------------------------

#[derive(Debug, Clone)]
struct ReserveSite {
    descriptors: u16,
    depth: u16,
    holder: String,
    span: Span,
    once_locally: bool,
}

#[derive(Debug, Clone)]
struct CallOccurrence {
    holder: String,
    once_locally: bool,
}

#[derive(Debug, Default)]
struct Facts {
    sites: Vec<ReserveSite>,
    callers: BTreeMap<String, Vec<CallOccurrence>>,
    edges: BTreeMap<String, BTreeSet<String>>,
    group_children: BTreeSet<String>,
}

// --- public entry ----------------------------------------------------------

pub(crate) fn check(programs: &BTreeMap<String, &TypedProgram>) -> Result<(), SemaError> {
    let facts = collect(programs);
    if facts.sites.is_empty() {
        return Ok(());
    }

    let descriptors = facts.sites[0].descriptors;
    for site in &facts.sites {
        if site.descriptors != descriptors {
            return Err(rejection(
                site.span,
                format!(
                    "this image's `reserve_proven` sites disagree on `descriptors=` \
                     ({descriptors} vs {}); machine v1's occupancy bound is \
                     `floor(queue_depth / descriptors_per_op)` for one descriptor \
                     count (03-hardware.md §4)",
                    site.descriptors
                ),
            ));
        }
        if site.descriptors != virtqueue::DESCRIPTORS_PER_BLK_OP {
            return Err(rejection(
                site.span,
                format!(
                    "`reserve_proven(descriptors={})`: machine v1's virtio-blk operation \
                     uses exactly {} descriptors (header + data + status); a different \
                     count would invent a second occupancy arithmetic",
                    site.descriptors,
                    virtqueue::DESCRIPTORS_PER_BLK_OP
                ),
            ));
        }
    }

    let depth = facts.sites[0].depth;
    for site in &facts.sites {
        if site.depth != depth {
            return Err(rejection(
                site.span,
                format!(
                    "this image's `reserve_proven` sites disagree on queue depth \
                     ({depth} vs {}); machine v1's `blk` has exactly one queue",
                    site.depth
                ),
            ));
        }
    }

    let occupancy = virtqueue::occupancy_bound(depth, descriptors);
    if occupancy == 0 {
        return Err(rejection(
            facts.sites[0].span,
            format!(
                "queue depth {depth} cannot hold even one {descriptors}-descriptor \
                 operation (occupancy bound is floor({depth}/{descriptors}) = 0)"
            ),
        ));
    }

    let count = facts.sites.len() as u16;
    if count > occupancy {
        return Err(rejection(
            facts.sites[0].span,
            format!(
                "queue depth {depth} admits at most {occupancy} concurrent \
                 {descriptors}-descriptor operations \
                 (floor({depth}/{descriptors})), but this image has {count} static \
                 `reserve_proven` site(s)"
            ),
        ));
    }

    let in_child = group_child_closure(&facts);
    let mut memo: BTreeMap<String, bool> = BTreeMap::new();
    for site in &facts.sites {
        if !site.once_locally {
            return Err(rejection(
                site.span,
                format!(
                    "a `reserve_proven` site in `{}` sits inside a loop or a closure \
                     body, so it can execute more than once per root turn",
                    site.holder
                ),
            ));
        }
        if in_child.contains(&site.holder) {
            return Err(rejection(
                site.span,
                format!(
                    "a `reserve_proven` site in `{}` sits inside a `g.start` child, \
                     which the at-most-once proof excludes (plans/M7.md decision 6: \
                     same shape as `sema::send_proof`)",
                    site.holder
                ),
            ));
        }
        if !at_most_once(
            &site.holder,
            &facts,
            &in_child,
            &mut memo,
            &mut BTreeSet::new(),
        ) {
            return Err(rejection(
                site.span,
                format!(
                    "a `reserve_proven` site in `{}` is not provably executed at most \
                     once per root turn — `{}` is reachable from more than one static \
                     call site, from a loop, or through a recursive cycle",
                    site.holder, site.holder
                ),
            ));
        }
    }
    Ok(())
}

fn rejection(span: Span, reason: String) -> SemaError {
    let mut e = SemaError::at(
        "type",
        "`reserve_proven` is not proven infallible for this image — every admitted \
         handler must be a complete unit against the queue's descriptor capacity \
         (03-hardware.md §4)"
            .to_string(),
        span,
    );
    e.extra_lines = vec![
        format!("  {reason}"),
        "  plans/M7.md decision 6: same analysis shape as `sema::send_proof`; \
         runtime backpressure is item G"
            .to_string(),
    ];
    e
}

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

// --- scan (send_proof's own walk, collecting reserve sites) ----------------

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
    once: bool,
    facts: &'a mut Facts,
}

impl Cx<'_> {
    fn note_call(&mut self, key: &CalleeKey, ordinary: bool) {
        let spelling = key.spelling();
        self.facts
            .callers
            .entry(spelling.clone())
            .or_default()
            .push(CallOccurrence {
                holder: self.holder.clone(),
                once_locally: self.once,
            });
        if ordinary {
            self.facts
                .edges
                .entry(self.holder.clone())
                .or_default()
                .insert(spelling);
        }
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
            TypedStmtKind::While { cond, body } => {
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
            TypedStmtKind::Defer(body) => match body {
                TypedDeferBody::Expr(e) => self.expr(e),
                TypedDeferBody::Suite(stmts) => self.stmts(stmts),
            },
            TypedStmtKind::ExprStmt(e) => self.expr(e),
            TypedStmtKind::BareSend { span: _, expr } => {
                if let TypedExprKind::Send(inner) = &expr.kind {
                    self.expr(inner);
                } else {
                    self.expr(expr);
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
            TypedExprKind::FnRef(key) => self.note_call(key, true),
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
                self.note_call(callee, true);
                if let Some(r) = receiver {
                    self.expr(r);
                }
                for a in args.iter().flatten() {
                    self.expr(a);
                }
            }
            TypedExprKind::CallValue(callee, args) => {
                self.expr(callee);
                for a in args {
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
                    self.note_call(key, true);
                }
            }
            TypedExprKind::Binary(_, l, r) | TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
                self.expr(l);
                self.expr(r);
            }
            TypedExprKind::OpCall(key, l, r) => {
                self.note_call(key, true);
                self.expr(l);
                self.expr(r);
            }
            TypedExprKind::Is(inner, pat) => {
                self.expr(inner);
                self.pattern(pat);
            }
            TypedExprKind::EnumConstruct { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
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
            TypedExprKind::Intrinsic {
                key,
                receiver,
                type_arg,
                args,
            } => {
                if key == "VirtQueue.reserve_proven" {
                    if let Some(site) = reserve_site_of(type_arg, args, &self.holder, self.once) {
                        self.facts.sites.push(site);
                    }
                }
                if let Some(r) = receiver {
                    self.expr(r);
                }
                for (_, a) in args {
                    self.expr(a);
                }
            }
            TypedExprKind::Await(inner) | TypedExprKind::Send(inner) => self.expr(inner),
            TypedExprKind::GroupChild(key) => {
                self.note_call(key, true);
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

/// `bodies::check_virtqueue_reserve_proven` stores the resolved depth in
/// `type_arg` as `VirtQueue[..<literal>]` (always an `Int` Bound, even when
/// the field's own annotation named a const), so the proof never has to
/// re-resolve a const.
fn reserve_site_of(
    type_arg: &Option<types::Type>,
    args: &[(String, TypedExpr)],
    holder: &str,
    once_locally: bool,
) -> Option<ReserveSite> {
    let ty = type_arg.as_ref()?;
    let types::Type::Named(name, targs) = ty else {
        return None;
    };
    if name != "VirtQueue" {
        return None;
    }
    let TypeArg::Bound(Expr::Int(span, text)) = targs.first()? else {
        return None;
    };
    let depth: u16 = text
        .chars()
        .filter(|c| *c != '_')
        .collect::<String>()
        .parse()
        .ok()?;
    let descriptors =
        args.iter()
            .find(|(l, _)| l == "descriptors")
            .and_then(|(_, v)| match &v.kind {
                TypedExprKind::Int(t) => t
                    .chars()
                    .filter(|c| *c != '_')
                    .collect::<String>()
                    .parse()
                    .ok(),
                _ => None,
            })?;
    Some(ReserveSite {
        descriptors,
        depth,
        holder: holder.to_string(),
        span: *span,
        once_locally,
    })
}
