pub mod image;
pub mod image_checks;
pub mod interp;
pub mod layout_assert;
pub mod legal;
pub mod observes;
pub mod quota;
pub mod value;
pub mod walk;

use crate::sema::SemaError;
use crate::sema::typed::{
    TestKind, TypedClosureBody, TypedDeferBody, TypedExpr, TypedFn, TypedForIter,
    TypedInstantiation, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::Type;
use crate::syntax::ast::Span;

pub use interp::EvalError;
pub use value::Value;

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

pub fn check_comptime(program: &TypedProgram) -> Result<(), SemaError> {
    let legality = legal::classify(program);
    check_comptime_asserts(program, &legality)?;
    check_consts(program, &legality)?;
    check_test_legality(program, &legality)?;
    check_image_legality(program, &legality)
}

fn check_image_legality(
    program: &TypedProgram,
    legality: &legal::Legality,
) -> Result<(), SemaError> {
    let Some(image_fn) = &program.image_fn else {
        return Ok(());
    };
    let Some(f) = program.fns.get(image_fn) else {
        return Ok(());
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

fn check_consts(program: &TypedProgram, legality: &legal::Legality) -> Result<(), SemaError> {
    for (name, c) in &program.consts {
        let scan = legal::scan_standalone(&c.value);
        for callee in &scan.callees {
            legal::require_legal(legality, callee, &format!("const {name}"), Span::default())?;
        }
        if let Some(reason) = &scan.illegal {
            return Err(SemaError::nowhere(
                "comptime",
                format!(
                    "`const {name}` requires a comptime-legal initializer, but it directly uses {reason}"
                ),
            ));
        }
        interp::eval_const(program, name).map_err(to_sema_error)?;
    }
    Ok(())
}

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

fn param_domain(program: &TypedProgram, ty: &Type) -> Option<Vec<Value>> {
    match ty {
        Type::Bool => Some(vec![Value::Bool(false), Value::Bool(true)]),
        Type::U8 => Some((0..=u8::MAX).map(Value::U8).collect()),
        Type::I8 => Some((i8::MIN..=i8::MAX).map(Value::I8).collect()),
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

struct AssertSite<'p> {
    cond: &'p TypedExpr,
    message: Option<&'p TypedExpr>,
    span: Span,
}

fn first_runtime_local(e: &TypedExpr) -> Option<&str> {
    use crate::sema::typed::TypedExprKind;
    match &e.kind {
        TypedExprKind::Local(name) => Some(name.as_str()),
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Const(_)
        | TypedExprKind::Static(_)
        | TypedExprKind::FnRef(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::GroupChild(_) => None,
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Try(base, _)
        | TypedExprKind::Is(base, _)
        | TypedExprKind::Panic(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base) => first_runtime_local(base),
        TypedExprKind::Index(a, b)
        | TypedExprKind::Binary(_, a, b)
        | TypedExprKind::OpCall(_, a, b)
        | TypedExprKind::And(a, b)
        | TypedExprKind::Or(a, b) => first_runtime_local(a).or_else(|| first_runtime_local(b)),
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                if let Some(n) = first_runtime_local(r) {
                    return Some(n);
                }
            }
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                if let Some(n) = first_runtime_local(a) {
                    return Some(n);
                }
            }
            None
        }
        TypedExprKind::CallValue(callee, args) => {
            if let Some(n) = first_runtime_local(callee) {
                return Some(n);
            }
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                if let Some(n) = first_runtime_local(a) {
                    return Some(n);
                }
            }
            None
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                if let Some(n) = first_runtime_local(a) {
                    return Some(n);
                }
            }
            None
        }
        TypedExprKind::Closure { .. } => None,
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                if let Some(n) = first_runtime_local(i) {
                    return Some(n);
                }
            }
            None
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                if let Some(n) = first_runtime_local(v) {
                    return Some(n);
                }
            }
            None
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                if let Some(n) = first_runtime_local(r) {
                    return Some(n);
                }
            }
            for (_, a) in args {
                if let Some(n) = first_runtime_local(a) {
                    return Some(n);
                }
            }
            None
        }
    }
}

fn check_one_comptime_assert(
    program: &TypedProgram,
    legality: &legal::Legality,
    site: AssertSite<'_>,
) -> Result<(), SemaError> {
    if let Some(name) = first_runtime_local(site.cond) {
        return Err(SemaError::at(
            "comptime",
            format!(
                "comptime assert condition references `{name}`, which is not comptime-\
                 visible here (only literals and top-level consts are — a local, a \
                 parameter, a loop variable, or a field of one cannot be)"
            ),
            site.span,
        ));
    }
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
        TypedStmtKind::While { cond, body, .. } => {
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
        | TypedExprKind::Static(_)
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
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                collect_asserts_expr(a, out);
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            collect_asserts_expr(callee, out);
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
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
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
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
        TypedExprKind::Await(inner) | TypedExprKind::Send(inner) => {
            collect_asserts_expr(inner, out)
        }
        TypedExprKind::GroupChild(_) => {}
    }
}
