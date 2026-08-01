use crate::sema::typed::{
    TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedForIter, TypedPattern,
    TypedPatternKind, TypedStmt, TypedStmtKind,
};

pub trait Visitor {
    fn pre_stmt(&mut self, _stmt: &TypedStmt) {}
    fn pre_expr(&mut self, _expr: &TypedExpr) {}
    fn on_callee(&mut self, _key: String) {}
    fn walk_patterns(&self) -> bool {
        false
    }
}

pub fn walk_stmts(stmts: &[TypedStmt], v: &mut dyn Visitor) {
    for s in stmts {
        walk_stmt(s, v);
    }
}

pub fn walk_stmt(stmt: &TypedStmt, v: &mut dyn Visitor) {
    v.pre_stmt(stmt);
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => walk_expr(value, v),
        TypedStmtKind::Assign { target, value } => {
            walk_expr(target, v);
            walk_expr(value, v);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            walk_expr(cond, v);
            walk_stmts(then_branch, v);
            for elif in elifs {
                walk_expr(&elif.cond, v);
                walk_stmts(&elif.body, v);
            }
            if let Some(b) = else_branch {
                walk_stmts(b, v);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, v);
            for arm in arms {
                if v.walk_patterns() {
                    walk_pattern(&arm.pattern, v);
                }
                if let Some(g) = &arm.guard {
                    walk_expr(g, v);
                }
                walk_stmts(&arm.body, v);
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(from, to, _) => {
                    walk_expr(from, v);
                    walk_expr(to, v);
                }
                TypedForIter::Expr(e) => walk_expr(e, v),
            }
            walk_stmts(body, v);
        }
        TypedStmtKind::While { cond, body, .. } => {
            walk_expr(cond, v);
            walk_stmts(body, v);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(value) => {
            if let Some(e) = value {
                walk_expr(e, v);
            }
        }
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            walk_expr(cond, v);
            if let Some(m) = message {
                walk_expr(m, v);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => walk_expr(e, v),
            TypedDeferBody::Suite(stmts) => walk_stmts(stmts, v),
        },
        TypedStmtKind::ExprStmt(e) => walk_expr(e, v),
        TypedStmtKind::BareSend { expr, .. } => walk_expr(expr, v),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                walk_expr(c, v);
            }
            if let Some(d) = deadline {
                walk_expr(d, v);
            }
            walk_stmts(body, v);
        }
    }
}

pub fn walk_expr(e: &TypedExpr, v: &mut dyn Visitor) {
    v.pre_expr(e);
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
        TypedExprKind::FnRef(key) => v.on_callee(key.spelling()),
        TypedExprKind::Field(base, _) => walk_expr(base, v),
        TypedExprKind::Index(base, idx) => {
            walk_expr(base, v);
            walk_expr(idx, v);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            v.on_callee(callee.spelling());
            if let Some(r) = receiver {
                walk_expr(r, v);
            }
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                walk_expr(a, v);
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            walk_expr(callee, v);
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                walk_expr(a, v);
            }
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Not(inner)
        | TypedExprKind::Panic(inner)
        | TypedExprKind::Await(inner)
        | TypedExprKind::Send(inner) => walk_expr(inner, v),
        TypedExprKind::Try(inner, conv) => {
            walk_expr(inner, v);
            if let Some(key) = conv {
                v.on_callee(key.spelling());
            }
        }
        TypedExprKind::Binary(_, l, r) | TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            walk_expr(l, v);
            walk_expr(r, v);
        }
        TypedExprKind::OpCall(key, l, r) => {
            v.on_callee(key.spelling());
            walk_expr(l, v);
            walk_expr(r, v);
        }
        TypedExprKind::Is(inner, pat) => {
            walk_expr(inner, v);
            if v.walk_patterns() {
                walk_pattern(pat, v);
            }
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                walk_expr(a, v);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => walk_expr(e, v),
            TypedClosureBody::Suite(stmts) => walk_stmts(stmts, v),
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                walk_expr(i, v);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, val) in fields {
                walk_expr(val, v);
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_expr(r, v);
            }
            for (_, a) in args {
                walk_expr(a, v);
            }
        }
        TypedExprKind::GroupChild(key) => v.on_callee(key.spelling()),
    }
}

pub fn walk_pattern(p: &TypedPattern, v: &mut dyn Visitor) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => walk_expr(e, v),
        TypedPatternKind::Take(inner) => walk_pattern(inner, v),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                walk_pattern(p, v);
            }
        }
        TypedPatternKind::Tuple(elems) | TypedPatternKind::Array(elems) => {
            for p in elems {
                walk_pattern(p, v);
            }
        }
        TypedPatternKind::Or(alts) => {
            for p in alts {
                walk_pattern(p, v);
            }
        }
    }
}
