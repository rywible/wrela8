use std::collections::{BTreeMap, BTreeSet};

use crate::eval::image::ImageGraph;
use crate::sema::SemaError;
use crate::sema::typed::{
    CalleeKey, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types;
use crate::syntax::ast::Span;

#[derive(Debug, Clone)]
struct MessageSite {
    actor: String,
    method: String,
    holder: String,
    bare: Option<Span>,
    once_locally: bool,
}

#[derive(Debug, Clone)]
struct CallOccurrence {
    holder: String,
    once_locally: bool,
    span: Span,
}

#[derive(Debug, Default)]
struct Facts {
    sites: Vec<MessageSite>,
    callers: BTreeMap<String, Vec<CallOccurrence>>,
    edges: BTreeMap<String, BTreeSet<String>>,
    sync_edges: BTreeMap<String, BTreeSet<String>>,
    group_children: BTreeSet<String>,
}

pub(crate) fn check(programs: &BTreeMap<String, &TypedProgram>) -> Result<(), SemaError> {
    let facts = collect(programs);
    check_sync_call_graph_acyclic(&facts, programs)?;
    if !facts.sites.iter().any(|s| s.bare.is_some()) {
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

fn check_sync_call_graph_acyclic(
    facts: &Facts,
    programs: &BTreeMap<String, &TypedProgram>,
) -> Result<(), SemaError> {
    let reachable = runtime_reachable(facts, programs);
    if reachable.is_empty() {
        return Ok(());
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        Grey,
        Black,
    }
    let mut colour: BTreeMap<&str, Colour> = BTreeMap::new();
    let mut path: Vec<&str> = Vec::new();

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

fn recursion_rejection(facts: &Facts, from: &str, to: &str, cycle: &[&str]) -> SemaError {
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

fn actor_capacities(programs: &BTreeMap<String, &TypedProgram>) -> Result<Capacities, String> {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, p)| p.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    let (module, fn_name) = match candidates.len() {
        0 => {
            return Err(
                "the build closure declares no `@image` fn, so no mailbox capacity is \
                         known — every mailbox bound this proof needs is declared there \
                         (`img.actor(T, mailbox=N)`)"
                    .to_string(),
            );
        }
        1 => candidates[0],
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
        let mailbox = mailbox.unwrap_or(0);
        caps.entry(name)
            .and_modify(|c| *c = (*c).min(mailbox))
            .or_insert(mailbox);
    }
    caps
}

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
            TypedStmtKind::BareSend { span, expr } => {
                let TypedExprKind::Send(inner) = &expr.kind else {
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
            TypedExprKind::Await(inner) => {
                if !self.note_message(inner, None) {
                    self.expr(inner);
                }
            }
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
