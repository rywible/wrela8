//! The `reserve` proof (plans/M7.md item E2, decision 6;
//! plans/M13.md item M / decision 1; 03-hardware.md §4).
//!
//! ## Proof-conditioned collapse (M13 decision 1)
//!
//! `VirtQueue.reserve` is spelled once. Its declared type is
//! `Result[QueuePermit, CapacityError]`. Where whole-image analysis
//! proves every admitted handler a complete unit against the queue's
//! descriptor capacity, use sites that expect `QueuePermit` may collapse
//! to that success type. Where the proof fails, those collapsed use
//! sites are refused with a why-chain (04-compiler.md §7 causality);
//! sites that keep the `Result` stay legal (and item L refuses silent
//! `Err` discard).
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
//! `VirtQueue.reserve` intrinsic.
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

    // Same live-driver filter as `collect`: QueuePermit demands from an
    // unused exporter `@driver` (loaded only because a layout was
    // imported) must not force a collapse failure on Tier 2 images.
    let mut imported_driver_names: BTreeSet<String> = BTreeSet::new();
    for program in programs.values() {
        for (name, s) in &program.imported.structs {
            if s.is_driver {
                imported_driver_names.insert(name.clone());
            }
        }
    }
    let mut demands: Vec<Span> = Vec::new();
    for program in programs.values() {
        let is_image_module = program.image_fn.is_some();
        let has_imported_driver = program
            .structs
            .iter()
            .any(|(n, s)| s.is_driver && imported_driver_names.contains(n));
        if !is_image_module && !has_imported_driver {
            continue;
        }
        demands.extend(program.reserve_permit_demands.iter().copied());
    }
    // Ill-formed images (disagreeing descriptors / depths, wrong
    // descriptor count) stay hard errors even when every site keeps the
    // Result — they invent a second occupancy arithmetic, not a capacity
    // failure mode.
    let descriptors = facts.sites[0].descriptors;
    for site in &facts.sites {
        if site.descriptors != descriptors {
            return Err(hard_rejection(
                site.span,
                format!(
                    "this image's `reserve` sites disagree on `descriptors=` \
                     ({descriptors} vs {}); machine v1's occupancy bound is \
                     `floor(queue_depth / descriptors_per_op)` for one descriptor \
                     count (03-hardware.md §4)",
                    site.descriptors
                ),
            ));
        }
        if site.descriptors != virtqueue::DESCRIPTORS_PER_BLK_OP {
            return Err(hard_rejection(
                site.span,
                format!(
                    "`reserve(descriptors={})`: machine v1's virtio-blk operation \
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
            return Err(hard_rejection(
                site.span,
                format!(
                    "this image's `reserve` sites disagree on queue depth \
                     ({depth} vs {}); machine v1's `blk` has exactly one queue",
                    site.depth
                ),
            ));
        }
    }

    let occupancy = virtqueue::occupancy_bound(depth, descriptors);
    let count = facts.sites.len() as u16;
    let in_child = group_child_closure(&facts);
    let mut memo: BTreeMap<String, bool> = BTreeMap::new();

    let mut why: Option<(Span, String)> = None;
    if occupancy == 0 {
        why = Some((
            facts.sites[0].span,
            format!(
                "queue depth {depth} cannot hold even one {descriptors}-descriptor \
                 operation (occupancy bound is floor({depth}/{descriptors}) = 0)"
            ),
        ));
    } else if count > occupancy {
        why = Some((
            facts.sites[0].span,
            format!(
                "queue depth {depth} admits at most {occupancy} concurrent \
                 {descriptors}-descriptor operations \
                 (floor({depth}/{descriptors})), but this image has {count} static \
                 `reserve` site(s)"
            ),
        ));
    } else {
        for site in &facts.sites {
            if !site.once_locally {
                why = Some((
                    site.span,
                    format!(
                        "a `reserve` site in `{}` sits inside a loop or a closure \
                         body, so it can execute more than once per root turn",
                        site.holder
                    ),
                ));
                break;
            }
            if in_child.contains(&site.holder) {
                why = Some((
                    site.span,
                    format!(
                        "a `reserve` site in `{}` sits inside a `g.start` child, \
                         which the at-most-once proof excludes (plans/M7.md decision 6: \
                         same shape as `sema::send_proof`)",
                        site.holder
                    ),
                ));
                break;
            }
            if !at_most_once(
                &site.holder,
                &facts,
                &in_child,
                &mut memo,
                &mut BTreeSet::new(),
            ) {
                why = Some((
                    site.span,
                    format!(
                        "a `reserve` site in `{}` is not provably executed at most \
                         once per root turn — `{}` is reachable from more than one static \
                         call site, from a loop, or through a recursive cycle",
                        site.holder, site.holder
                    ),
                ));
                break;
            }
        }
    }

    if let Some((fact_span, reason)) = why {
        // No QueuePermit collapse demanded → Result typing is fine;
        // item L covers silent discard of CapacityError.
        if demands.is_empty() {
            return Ok(());
        }
        let demand_span = demands[0];
        return Err(collapse_rejection(
            demand_span,
            fact_span,
            depth,
            descriptors,
            occupancy,
            count,
            reason,
        ));
    }
    Ok(())
}

fn hard_rejection(span: Span, reason: String) -> SemaError {
    let mut e = SemaError::at("type", reason, span);
    e.extra_lines = vec![
        "  plans/M7.md decision 6 / plans/M13.md item M: occupancy arithmetic \
         is one image fact (03-hardware.md §4)"
            .to_string(),
    ];
    e
}

/// Failed proof at a use site that demanded `QueuePermit` — why-chain
/// per 04-compiler.md §7 (queue, depth, in-flight bound, image fact).
fn collapse_rejection(
    demand_span: Span,
    fact_span: Span,
    depth: u16,
    descriptors: u16,
    occupancy: u16,
    site_count: u16,
    reason: String,
) -> SemaError {
    let _ = fact_span;
    let mut e = SemaError::at(
        "type",
        "`reserve` is not proven infallible for this image — every admitted \
         handler must be a complete unit against the queue's descriptor capacity \
         (03-hardware.md §4); the use site demanded `QueuePermit` (plans/M13.md \
         decision 1)"
            .to_string(),
        demand_span,
    );
    e.extra_lines = vec![
        format!("  queue: VirtQueue[..{depth}]"),
        format!("  descriptors_per_op: {descriptors}"),
        format!("  in-flight bound: floor({depth}/{descriptors}) = {occupancy}"),
        format!("  static `reserve` sites in image: {site_count}"),
        format!("  image fact: {reason}"),
        "  04-compiler.md §7: diagnostics carry a why-chain for whole-image analyses".to_string(),
        "  plans/M13.md item M / decision 1: keep `Result[QueuePermit, CapacityError]` \
         at the use site, or shrink the image until the proof holds"
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
    // plans/M16.md Wave D Tier 2: importing a *layout/type* from
    // `drivers.blk` loads the whole exporter, including an unused
    // `@driver BlkDriver`. Do not count that driver's `reserve` sites
    // against an image that keeps its own specialized local driver of
    // the same bare name. Count a `@driver` when:
    //   - it is declared in a module that has `@image`, or
    //   - some module imports it by name (Tier 1 `import BlkDriver`).
    let imported_driver_names: BTreeSet<String> = programs
        .values()
        .flat_map(|p| {
            p.imported
                .structs
                .iter()
                .filter(|(_, s)| s.is_driver)
                .map(|(n, _)| n.clone())
        })
        .collect();

    let mut facts = Facts::default();
    for program in programs.values() {
        for (name, f) in &program.fns {
            scan_fn(name.clone(), f, &mut facts);
        }
        for (struct_name, s) in &program.structs {
            if s.is_driver {
                let live = program.image_fn.is_some()
                    || imported_driver_names.contains(struct_name);
                if !live {
                    continue;
                }
            }
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
            TypedStmtKind::While { cond, body, .. } => {
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
                for a in args.iter().filter_map(|a| a.value.as_ref()) {
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
                if key == "VirtQueue.reserve" {
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

/// `bodies::check_virtqueue_reserve` stores the resolved depth in
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
