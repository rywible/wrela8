use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::sema::bodies::{
    self, FnInfo, InstKind, MAX_GENERIC_DEPTH, ModuleCtx, QueuedInstantiation, StructInfo,
};
use crate::sema::typed::{TypedEnum, TypedInstantiation};
use crate::sema::types::{
    self, Classification, DeclEnum, DeclField, DeclFn, DeclGenericKind, DeclGenericParam,
    DeclMember, DeclParam, DeclStruct, DeclVariant, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, access, flow, matches, unimplemented_at};
use crate::syntax::ast::{self, Arg, BinOp, ClosureBody, Expr, Member, Module, Span, Stmt};
use crate::syntax::printer;

pub(crate) fn canonical_key(kind: InstKind, name: &str, args: &[TypeArg]) -> String {
    debug_assert_ne!(
        kind,
        InstKind::Method,
        "method keys use canonical_method_key"
    );
    format!("{}:{}", kind.tag(), display_name(name, args))
}

pub(crate) fn canonical_method_key(receiver: &Type, method: &str, args: &[TypeArg]) -> String {
    format!(
        "method:{}.{}",
        types::render_type(receiver),
        display_name(method, args)
    )
}

fn display_name(name: &str, args: &[TypeArg]) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let rendered: Vec<String> = args.iter().map(types::render_type_arg).collect();
    format!("{name}[{}]", rendered.join(", "))
}

fn display_inst_name(entry: &QueuedInstantiation) -> String {
    match (&entry.kind, &entry.receiver) {
        (InstKind::Method, Some(recv)) => {
            format!(
                "{}.{}",
                types::render_type(recv),
                display_name(&entry.name, &entry.args)
            )
        }
        _ => display_name(&entry.name, &entry.args),
    }
}

#[derive(Debug, Clone, Default)]
struct Subst {
    types: BTreeMap<String, Type>,
    consts: BTreeMap<String, Expr>,
}

fn subst_type(ty: &Type, subst: &Subst) -> Type {
    match ty {
        Type::Generic(name) => subst.types.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(elem, len) => Type::Array(
            Box::new(subst_type(elem, subst)),
            Box::new(subst_expr(len, subst)),
        ),
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|t| subst_type(t, subst)).collect()),
        Type::Option(inner) => Type::Option(Box::new(subst_type(inner, subst))),
        Type::Result(ok, err) => Type::Result(
            Box::new(subst_type(ok, subst)),
            Box::new(subst_type(err, subst)),
        ),
        Type::Own(pool, inner) => Type::Own(pool.clone(), Box::new(subst_type(inner, subst))),
        Type::Static(inner) => Type::Static(Box::new(subst_type(inner, subst))),
        Type::Bytes(Some(len)) => Type::Bytes(Some(Box::new(subst_expr(len, subst)))),
        Type::Bytes(None) => Type::Bytes(None),
        Type::String(len) => Type::String(Box::new(subst_expr(len, subst))),
        Type::Fn(params, ret) => Type::Fn(
            params
                .iter()
                .map(|(m, t)| (*m, subst_type(t, subst)))
                .collect(),
            Box::new(subst_type(ret, subst)),
        ),
        Type::Named(name, targs) => Type::Named(
            name.clone(),
            targs.iter().map(|a| subst_type_arg(a, subst)).collect(),
        ),
        other => other.clone(),
    }
}

fn subst_type_arg(arg: &TypeArg, subst: &Subst) -> TypeArg {
    match arg {
        TypeArg::Type(t) => TypeArg::Type(subst_type(t, subst)),
        TypeArg::Const(e) => TypeArg::Const(subst_expr(e, subst)),
        TypeArg::Bound(e) => TypeArg::Bound(subst_expr(e, subst)),
        TypeArg::Pool(p) => TypeArg::Pool(p.clone()),
    }
}

pub(crate) fn instantiate_enum_payload_types(
    enumeration: &TypedEnum,
    args: &[TypeArg],
) -> Option<Vec<Vec<Type>>> {
    if enumeration.generic_type_params.len() != args.len() {
        return None;
    }
    let mut subst = Subst::default();
    for (parameter, argument) in enumeration.generic_type_params.iter().zip(args) {
        match (parameter, argument) {
            (Some(parameter), TypeArg::Type(argument)) => {
                subst.types.insert(parameter.clone(), argument.clone());
            }
            // TypedEnum deliberately retains no const-generic parameter name.
            // Refuse to synthesize a layout that cannot be substituted exactly.
            (None, _) | (Some(_), _) => return None,
        }
    }
    Some(
        enumeration
            .variant_payload_types
            .iter()
            .map(|payload| payload.iter().map(|ty| subst_type(ty, &subst)).collect())
            .collect(),
    )
}

fn subst_expr(e: &Expr, subst: &Subst) -> Expr {
    match e {
        Expr::Name(span, name) => subst
            .consts
            .get(name)
            .cloned()
            .or_else(|| {
                subst
                    .types
                    .get(name)
                    .and_then(|ty| type_as_call_arg_expr(ty, *span))
            })
            .unwrap_or_else(|| e.clone()),
        Expr::Field(base, span, name) => {
            Expr::Field(Box::new(subst_expr(base, subst)), *span, name.clone())
        }
        Expr::Index(base, span, args) => Expr::Index(
            Box::new(subst_expr(base, subst)),
            *span,
            args.iter().map(|a| subst_expr(a, subst)).collect(),
        ),
        Expr::Call(callee, span, args) => Expr::Call(
            Box::new(subst_expr(callee, subst)),
            *span,
            args.iter()
                .map(|a| Arg {
                    span: a.span,
                    label: a.label.clone(),
                    mode: a.mode,
                    value: subst_expr(&a.value, subst),
                })
                .collect(),
        ),
        Expr::Unary(span, op, inner) => Expr::Unary(*span, *op, Box::new(subst_expr(inner, subst))),
        Expr::Try(span, inner) => Expr::Try(*span, Box::new(subst_expr(inner, subst))),
        Expr::Binary(span, op, l, r) => Expr::Binary(
            *span,
            *op,
            Box::new(subst_expr(l, subst)),
            Box::new(subst_expr(r, subst)),
        ),
        Expr::Range(span, f, t, incl) => Expr::Range(
            *span,
            Box::new(subst_expr(f, subst)),
            Box::new(subst_expr(t, subst)),
            *incl,
        ),
        Expr::Is(span, inner, pat) => {
            Expr::Is(*span, Box::new(subst_expr(inner, subst)), pat.clone())
        }
        Expr::Not(span, inner) => Expr::Not(*span, Box::new(subst_expr(inner, subst))),
        Expr::And(span, l, r) => Expr::And(
            *span,
            Box::new(subst_expr(l, subst)),
            Box::new(subst_expr(r, subst)),
        ),
        Expr::Or(span, l, r) => Expr::Or(
            *span,
            Box::new(subst_expr(l, subst)),
            Box::new(subst_expr(r, subst)),
        ),
        Expr::DotVariant(span, name, args) => Expr::DotVariant(
            *span,
            name.clone(),
            args.iter()
                .map(|a| Arg {
                    span: a.span,
                    label: a.label.clone(),
                    mode: a.mode,
                    value: subst_expr(&a.value, subst),
                })
                .collect(),
        ),
        Expr::Send(span, inner) => Expr::Send(*span, Box::new(subst_expr(inner, subst))),
        Expr::Tuple(span, items) => {
            Expr::Tuple(*span, items.iter().map(|i| subst_expr(i, subst)).collect())
        }
        Expr::List(span, items) => {
            Expr::List(*span, items.iter().map(|i| subst_expr(i, subst)).collect())
        }
        Expr::ArrayRepeat(span, elem, count) => Expr::ArrayRepeat(
            *span,
            Box::new(subst_expr(elem, subst)),
            Box::new(subst_expr(count, subst)),
        ),
        Expr::Closure(c) => Expr::Closure(crate::syntax::ast::ClosureExpr {
            body: match &c.body {
                ClosureBody::Expr(e) => ClosureBody::Expr(Box::new(subst_expr(e, subst))),
                ClosureBody::Suite(stmts) => {
                    ClosureBody::Suite(stmts.iter().map(|s| subst_stmt(s, subst)).collect())
                }
            },
            ..c.clone()
        }),
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Str(_, _)
        | Expr::BStr(_, _)
        | Expr::Char(_, _)
        | Expr::FStr(_)
        | Expr::Bool(_, _)
        | Expr::Unit(_) => e.clone(),
    }
}

fn type_as_call_arg_expr(ty: &Type, span: Span) -> Option<Expr> {
    match ty {
        Type::Generic(name) => Some(Expr::Name(span, name.clone())),
        Type::Named(name, args) => {
            let base = Expr::Name(span, name.clone());
            if args.is_empty() {
                return Some(base);
            }
            let args = args
                .iter()
                .map(|argument| match argument {
                    TypeArg::Type(ty) => type_as_call_arg_expr(ty, span),
                    TypeArg::Const(expr) | TypeArg::Bound(expr) => Some(expr.clone()),
                    TypeArg::Pool(name) => Some(Expr::Name(span, name.clone())),
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Expr::Index(Box::new(base), span, args))
        }
        Type::Bool => Some(Expr::Name(span, "bool".to_string())),
        Type::U8 => Some(Expr::Name(span, "u8".to_string())),
        Type::U16 => Some(Expr::Name(span, "u16".to_string())),
        Type::U32 => Some(Expr::Name(span, "u32".to_string())),
        Type::U64 => Some(Expr::Name(span, "u64".to_string())),
        Type::I8 => Some(Expr::Name(span, "i8".to_string())),
        Type::I16 => Some(Expr::Name(span, "i16".to_string())),
        Type::I32 => Some(Expr::Name(span, "i32".to_string())),
        Type::I64 => Some(Expr::Name(span, "i64".to_string())),
        Type::F32 => Some(Expr::Name(span, "f32".to_string())),
        Type::F64 => Some(Expr::Name(span, "f64".to_string())),
        Type::Char => Some(Expr::Name(span, "char".to_string())),
        _ => None,
    }
}

fn subst_stmt(s: &Stmt, subst: &Subst) -> Stmt {
    match s {
        Stmt::Assign(a) => Stmt::Assign(crate::syntax::ast::AssignStmt {
            target: subst_expr(&a.target, subst),
            value: subst_expr(&a.value, subst),
            ..a.clone()
        }),
        Stmt::If(i) => Stmt::If(crate::syntax::ast::IfStmt {
            cond: subst_expr(&i.cond, subst),
            then_branch: i.then_branch.iter().map(|s| subst_stmt(s, subst)).collect(),
            elifs: i
                .elifs
                .iter()
                .map(|e| crate::syntax::ast::ElifClause {
                    cond: subst_expr(&e.cond, subst),
                    body: e.body.iter().map(|s| subst_stmt(s, subst)).collect(),
                    ..e.clone()
                })
                .collect(),
            else_branch: i
                .else_branch
                .as_ref()
                .map(|b| b.iter().map(|s| subst_stmt(s, subst)).collect()),
            ..i.clone()
        }),
        Stmt::Match(m) => Stmt::Match(crate::syntax::ast::MatchStmt {
            scrutinee: subst_expr(&m.scrutinee, subst),
            arms: m
                .arms
                .iter()
                .map(|a| crate::syntax::ast::MatchArm {
                    guard: a.guard.as_ref().map(|g| subst_expr(g, subst)),
                    body: a.body.iter().map(|s| subst_stmt(s, subst)).collect(),
                    ..a.clone()
                })
                .collect(),
            ..m.clone()
        }),
        Stmt::For(f) => Stmt::For(crate::syntax::ast::ForStmt {
            iterable: subst_expr(&f.iterable, subst),
            body: f.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..f.clone()
        }),
        Stmt::While(w) => Stmt::While(crate::syntax::ast::WhileStmt {
            cond: subst_expr(&w.cond, subst),
            body: w.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..w.clone()
        }),
        Stmt::Return(span, Some(e)) => Stmt::Return(*span, Some(subst_expr(e, subst))),
        Stmt::Assert(a) => Stmt::Assert(crate::syntax::ast::AssertStmt {
            cond: subst_expr(&a.cond, subst),
            message: a.message.as_ref().map(|m| subst_expr(m, subst)),
            ..a.clone()
        }),
        Stmt::Defer(d) => Stmt::Defer(crate::syntax::ast::DeferStmt {
            body: match &d.body {
                crate::syntax::ast::DeferBody::Expr(e) => {
                    crate::syntax::ast::DeferBody::Expr(Box::new(subst_expr(e, subst)))
                }
                crate::syntax::ast::DeferBody::Suite(stmts) => {
                    crate::syntax::ast::DeferBody::Suite(
                        stmts.iter().map(|s| subst_stmt(s, subst)).collect(),
                    )
                }
            },
            ..d.clone()
        }),
        Stmt::With(w) => Stmt::With(crate::syntax::ast::WithStmt {
            expr: subst_expr(&w.expr, subst),
            body: w.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..w.clone()
        }),
        Stmt::Send(span, e) => Stmt::Send(*span, subst_expr(e, subst)),
        Stmt::Expr(span, e) => Stmt::Expr(*span, subst_expr(e, subst)),
        Stmt::ComptimeAssert(span, cond, msg) => Stmt::ComptimeAssert(
            *span,
            subst_expr(cond, subst),
            msg.as_ref().map(|m| subst_expr(m, subst)),
        ),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::Return(_, None)
        | Stmt::Pass(_)
        | Stmt::Dmb(_)
        | Stmt::ComptimeIf(_) => s.clone(),
    }
}

fn subst_member_ast(m: &Member, subst: &Subst) -> Member {
    match m {
        Member::Fn(f) => Member::Fn(crate::syntax::ast::FnItem {
            body: f
                .body
                .as_ref()
                .map(|b| b.iter().map(|s| subst_stmt(s, subst)).collect()),
            ..f.clone()
        }),
        Member::Init(i) => Member::Init(crate::syntax::ast::InitItem {
            body: i.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..i.clone()
        }),
        Member::Field(f) => Member::Field(crate::syntax::ast::FieldItem {
            default: f.default.as_ref().map(|d| subst_expr(d, subst)),
            ..f.clone()
        }),
        Member::Pool(_) | Member::ComptimeIf(_) => m.clone(),
    }
}

fn renderer_vector_components<'a>(ty: &Type, mctx: &ModuleCtx) -> Option<&'a [&'a str]> {
    let Type::Named(name, args) = ty else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let declaration = mctx.type_decl_name.get(name)?;
    let module = mctx.type_decl_module.get(name)?;
    if !matches!(module.as_str(), "field" | "core.field") {
        return None;
    }
    Some(match declaration.as_str() {
        "Vec2" => &["x", "y"],
        "Vec3" => &["x", "y", "z"],
        "Vec4" => &["x", "y", "z", "w"],
        "Rgb" => &["r", "g", "b"],
        _ => return None,
    })
}

fn renderer_numeric_literal(value: f64, ty: &Type) -> String {
    let mut literal = match ty {
        Type::F32 => (value as f32).to_string(),
        Type::F64 => value.to_string(),
        _ => value.to_string(),
    };
    if matches!(ty, Type::F32 | Type::F64)
        && !literal.contains('.')
        && !literal.contains('e')
        && !literal.contains('E')
    {
        literal.push_str(".0");
    }
    literal
}

fn write_renderer_leaf_check(
    output: &mut String,
    indent: usize,
    path: &str,
    ty: &Type,
    range: crate::sema::attrs::NumericRange,
    component: &mut u32,
) {
    let spaces = " ".repeat(indent);
    let path_component = *component;
    *component = component.saturating_add(1);
    if matches!(ty, Type::F32 | Type::F64) {
        let max_finite = if *ty == Type::F32 {
            "3.4028234663852886e38"
        } else {
            "1.7976931348623157e308"
        };
        for condition in [
            format!("{path} != {path}"),
            format!("{path} > {max_finite}"),
            format!("{path} < -{max_finite}"),
        ] {
            writeln!(output, "{spaces}if {condition}:").expect("String writes cannot fail");
            writeln!(
                output,
                "{spaces}    return Err(RenderError.NonFiniteInput(RenderPath(component={path_component})))"
            )
            .expect("String writes cannot fail");
        }
    }
    let (min, max) = if let Some((min, max)) = range.exact_integer {
        (min.to_string(), max.to_string())
    } else {
        (
            renderer_numeric_literal(range.min, ty),
            renderer_numeric_literal(range.max, ty),
        )
    };
    for condition in [format!("{path} < {min}"), format!("{path} > {max}")] {
        writeln!(output, "{spaces}if {condition}:").expect("String writes cannot fail");
        writeln!(
            output,
            "{spaces}    return Err(RenderError.ParameterOutOfRange(RenderPath(component={path_component})))"
        )
        .expect("String writes cannot fail");
    }
}

fn write_renderer_type_checks(
    output: &mut String,
    indent: usize,
    path: &str,
    ty: &Type,
    explicit_range: Option<crate::sema::attrs::NumericRange>,
    component: &mut u32,
    loop_id: &mut u32,
    depth: usize,
    active: &mut BTreeSet<String>,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    if depth > MAX_GENERIC_DEPTH {
        return Err(SemaError::at(
            "pixels P5",
            "renderer frame-validation type nesting exceeds the generic depth ceiling".to_string(),
            Span::default(),
        ));
    }
    if let Some(range) = explicit_range {
        if matches!(
            ty,
            Type::F32
                | Type::F64
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::I64
                | Type::Isize
        ) {
            write_renderer_leaf_check(output, indent, path, ty, range, component);
            return Ok(());
        }
        if let Some(components) = renderer_vector_components(ty, mctx) {
            for field in components {
                write_renderer_leaf_check(
                    output,
                    indent,
                    &format!("{path}.{field}"),
                    &Type::F32,
                    range,
                    component,
                );
            }
            return Ok(());
        }
    }
    match ty {
        Type::Array(element, length) => {
            let Some(length) = bodies::literal_array_len(length) else {
                return Err(SemaError::at(
                    "pixels P5",
                    "renderer frame-validation array length is not an exact literal".to_string(),
                    length.span(),
                ));
            };
            let mut nested = String::new();
            let this_loop = *loop_id;
            *loop_id = loop_id.saturating_add(1);
            let index = format!("__wrela_p5_index_{this_loop}");
            write_renderer_type_checks(
                &mut nested,
                indent + 4,
                &format!("{path}[{index}]"),
                element,
                None,
                component,
                loop_id,
                depth + 1,
                active,
                mctx,
            )?;
            if !nested.is_empty() {
                let spaces = " ".repeat(indent);
                let end = format!("__wrela_p5_end_{this_loop}");
                writeln!(output, "{spaces}{end}: usize = {length}")
                    .expect("String writes cannot fail");
                writeln!(output, "{spaces}@budget(bound={})", length.max(1))
                    .expect("String writes cannot fail");
                writeln!(output, "{spaces}for {index} in 0 .. {end}:")
                    .expect("String writes cannot fail");
                output.push_str(&nested);
            }
        }
        Type::Named(name, args) => {
            if renderer_vector_components(ty, mctx).is_some() || mctx.enums.contains_key(name) {
                return Ok(());
            }
            let key = types::render_type(ty);
            if !active.insert(key.clone()) {
                return Ok(());
            }
            let info = if args.is_empty() {
                mctx.structs.get(name).cloned()
            } else {
                Some(instantiate_struct(mctx, name, args, Span::default())?)
            };
            if let Some(info) = info {
                for (ast_member, decl_member) in info.members() {
                    let (Member::Field(field), DeclMember::Field(decl_field)) =
                        (ast_member, decl_member)
                    else {
                        continue;
                    };
                    let range = crate::sema::attrs::parse_field_contracts(
                        field,
                        &decl_field.ty,
                        &mctx.const_values,
                        renderer_vector_components(&decl_field.ty, mctx).is_some(),
                    )?
                    .range;
                    write_renderer_type_checks(
                        output,
                        indent,
                        &format!("{path}.{}", field.name),
                        &decl_field.ty,
                        range,
                        component,
                        loop_id,
                        depth + 1,
                        active,
                        mctx,
                    )?;
                }
            }
            active.remove(&key);
        }
        Type::Tuple(items) => {
            for (index, item) in items.iter().enumerate() {
                write_renderer_type_checks(
                    output,
                    indent,
                    &format!("{path}.{index}"),
                    item,
                    None,
                    component,
                    loop_id,
                    depth + 1,
                    active,
                    mctx,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn renderer_parameter_validation(
    parameter_type: &Type,
    mctx: &ModuleCtx,
) -> Result<Vec<Stmt>, SemaError> {
    let mut body = String::new();
    let mut component = 0x1000_u32;
    let mut loop_id = 0_u32;
    write_renderer_type_checks(
        &mut body,
        4,
        "frame.params",
        parameter_type,
        None,
        &mut component,
        &mut loop_id,
        0,
        &mut BTreeSet::new(),
        mctx,
    )?;
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let source = format!("module __wrela_renderer_validation\n\nfn checks():\n{body}    pass\n");
    let tokens = crate::syntax::lexer::lex(&source).map_err(|error| {
        SemaError::at(
            "pixels P5",
            format!(
                "compiler-generated renderer validation did not lex: {}",
                error.message
            ),
            Span::default(),
        )
    })?;
    let module = crate::syntax::parser::parse(tokens).map_err(|error| {
        SemaError::at(
            "pixels P5",
            format!(
                "compiler-generated renderer validation did not parse: {}",
                error.message
            ),
            Span::default(),
        )
    })?;
    let Some(ast::Item::Fn(function)) = module.items.into_iter().next() else {
        return Err(SemaError::at(
            "pixels P5",
            "compiler-generated renderer validation has no function body".to_string(),
            Span::default(),
        ));
    };
    let mut statements = function.body.unwrap_or_default();
    if matches!(statements.last(), Some(Stmt::Pass(_))) {
        statements.pop();
    }
    Ok(statements)
}

fn renderer_snapshot_numeric(ty: &Type) -> bool {
    matches!(
        ty,
        Type::F32
            | Type::F64
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::Isize
    )
}

fn write_renderer_snapshot_scalar(
    output: &mut String,
    source: &str,
    path: &[usize],
    component: Option<u8>,
    keys: &mut BTreeSet<u64>,
) -> Result<(), SemaError> {
    let key = crate::pixels::params::parameter_path_key(path, component)
        .map_err(|message| SemaError::at("pixels P7", message, Span::default()))?;
    if !keys.insert(key) {
        return Err(SemaError::at(
            "pixels P7",
            format!(
                "renderer snapshot path-key collision for parameter path {path:?} component {component:?}"
            ),
            Span::default(),
        ));
    }
    writeln!(
        output,
        "    slot_info = __wrela_pixels_p7_param_slot(self.renderer_index.to[usize](), {key})\n\
         \x20   if slot_info[0] == 1:\n\
         \x20       if slot_info[1] != used_count.to[u64]() or used_count >= 16:\n\
         \x20           return FrameInputSnapshot.make_frame(params=values, param_count=65535, camera=frame.camera, lights=frame.lights, exposure=frame.exposure, environment=frame.environment, frame_index=frame.frame_index)\n\
         \x20       values[used_count.to[usize]()] = {source}.to[f32]()\n\
         \x20       used_count = used_count + 1"
    )
    .expect("String writes cannot fail");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_renderer_snapshot_values(
    output: &mut String,
    path: &str,
    ty: &Type,
    explicit_range: Option<crate::sema::attrs::NumericRange>,
    field_path: &mut Vec<usize>,
    keys: &mut BTreeSet<u64>,
    depth: usize,
    active: &mut BTreeSet<String>,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    if depth > MAX_GENERIC_DEPTH {
        return Err(SemaError::at(
            "pixels P7",
            "renderer snapshot type nesting exceeds the generic depth ceiling".to_string(),
            Span::default(),
        ));
    }
    if explicit_range.is_some() && renderer_snapshot_numeric(ty) {
        return write_renderer_snapshot_scalar(output, path, field_path, None, keys);
    }
    if explicit_range.is_some()
        && let Some(components) = renderer_vector_components(ty, mctx)
    {
        for (component_index, component) in components.into_iter().enumerate() {
            write_renderer_snapshot_scalar(
                output,
                &format!("{path}.{component}"),
                field_path,
                Some(
                    u8::try_from(component_index)
                        .expect("renderer vectors have at most four components"),
                ),
                keys,
            )?;
        }
        return Ok(());
    }
    match ty {
        Type::Array(element, length) => {
            let Some(length) = bodies::literal_array_len(length) else {
                return Err(SemaError::at(
                    "pixels P7",
                    "renderer snapshot array length is not an exact literal".to_string(),
                    length.span(),
                ));
            };
            for index in 0..length {
                field_path.push(usize::try_from(index).map_err(|_| {
                    SemaError::at(
                        "pixels P7",
                        "renderer snapshot array index exceeds usize".to_string(),
                        Span::default(),
                    )
                })?);
                write_renderer_snapshot_values(
                    output,
                    &format!("{path}[{index}]"),
                    element,
                    None,
                    field_path,
                    keys,
                    depth + 1,
                    active,
                    mctx,
                )?;
                field_path.pop();
            }
        }
        Type::Named(name, args) => {
            if renderer_vector_components(ty, mctx).is_some() || mctx.enums.contains_key(name) {
                return Ok(());
            }
            let key = types::render_type(ty);
            if !active.insert(key.clone()) {
                return Ok(());
            }
            let info = if args.is_empty() {
                mctx.structs.get(name).cloned()
            } else {
                Some(instantiate_struct(mctx, name, args, Span::default())?)
            };
            if let Some(info) = info {
                let mut field_index = 0_usize;
                for (ast_member, decl_member) in info.members() {
                    let (Member::Field(field), DeclMember::Field(decl_field)) =
                        (ast_member, decl_member)
                    else {
                        continue;
                    };
                    let range = crate::sema::attrs::parse_field_contracts(
                        field,
                        &decl_field.ty,
                        &mctx.const_values,
                        renderer_vector_components(&decl_field.ty, mctx).is_some(),
                    )?
                    .range;
                    field_path.push(field_index);
                    write_renderer_snapshot_values(
                        output,
                        &format!("{path}.{}", field.name),
                        &decl_field.ty,
                        range,
                        field_path,
                        keys,
                        depth + 1,
                        active,
                        mctx,
                    )?;
                    field_path.pop();
                    field_index += 1;
                }
            }
            active.remove(&key);
        }
        Type::Tuple(items) => {
            for (index, item) in items.iter().enumerate() {
                field_path.push(index);
                write_renderer_snapshot_values(
                    output,
                    &format!("{path}.{index}"),
                    item,
                    None,
                    field_path,
                    keys,
                    depth + 1,
                    active,
                    mctx,
                )?;
                field_path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

fn renderer_snapshot_body(parameter_type: &Type, mctx: &ModuleCtx) -> Result<Vec<Stmt>, SemaError> {
    let mut body = String::from("    values: [f32; 16] = [0.0; 16]\n    used_count: u16 = 0\n");
    write_renderer_snapshot_values(
        &mut body,
        "frame.params",
        parameter_type,
        None,
        &mut Vec::new(),
        &mut BTreeSet::new(),
        0,
        &mut BTreeSet::new(),
        mctx,
    )?;
    writeln!(
        body,
        "    return FrameInputSnapshot.make_frame(params=values, param_count=used_count, camera=frame.camera, lights=frame.lights, exposure=frame.exposure, environment=frame.environment, frame_index=frame.frame_index)"
    )
    .expect("String writes cannot fail");
    let source = format!("module __wrela_renderer_snapshot\n\nfn snapshot():\n{body}");
    let tokens = crate::syntax::lexer::lex(&source).map_err(|error| {
        SemaError::at(
            "pixels P7",
            format!(
                "compiler-generated renderer snapshot did not lex: {}",
                error.message
            ),
            Span::default(),
        )
    })?;
    let module = crate::syntax::parser::parse(tokens).map_err(|error| {
        SemaError::at(
            "pixels P7",
            format!(
                "compiler-generated renderer snapshot did not parse: {}",
                error.message
            ),
            Span::default(),
        )
    })?;
    let Some(ast::Item::Fn(function)) = module.items.into_iter().next() else {
        return Err(SemaError::at(
            "pixels P7",
            "compiler-generated renderer snapshot has no function body".to_string(),
            Span::default(),
        ));
    };
    Ok(function.body.unwrap_or_default())
}

fn build_consts_program(mctx: &ModuleCtx) -> Result<crate::sema::typed::TypedProgram, SemaError> {
    let mut program = crate::sema::typed::TypedProgram::default();
    for (name, ty) in &mctx.consts {
        let Some(raw) = mctx.const_values.get(name) else {
            continue;
        };
        if contains_generic_brackets(raw) {
            continue;
        }
        let mut fctx = bodies::FnCtx::new(Type::Unit, mctx.module_pools.clone());
        if let Ok(value) = bodies::check_expr(raw, Some(ty), &mut fctx, mctx) {
            program.consts.insert(
                name.clone(),
                crate::sema::typed::TypedConst {
                    ty: ty.clone(),
                    value,
                },
            );
        }
    }
    for (name, en) in &mctx.enums {
        if en.generics.is_empty() {
            program.enums.insert(
                name.clone(),
                TypedEnum::from_variants(en.variants.iter().map(|v| v.name.clone()).collect()),
            );
        }
    }
    for name in ["Target", "Transport", "Failure", "DriverMode"] {
        if let Some(vs) = crate::sema::stdlib_enums::variant_strs(name)? {
            program.enums.entry(name.to_string()).or_insert_with(|| {
                TypedEnum::from_variants(vs.iter().map(|v| v.to_string()).collect())
            });
        }
    }
    Ok(program)
}

fn contains_generic_brackets(e: &Expr) -> bool {
    match e {
        Expr::Index(..) => true,
        Expr::Name(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(..)
        | Expr::FStr(_) => false,
        Expr::Field(base, _, _) => contains_generic_brackets(base),
        Expr::Call(callee, _, args) => {
            contains_generic_brackets(callee)
                || args.iter().any(|a| contains_generic_brackets(&a.value))
        }
        Expr::Unary(_, _, inner) | Expr::Try(_, inner) | Expr::Not(_, inner) => {
            contains_generic_brackets(inner)
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            contains_generic_brackets(l) || contains_generic_brackets(r)
        }
        Expr::Range(_, a, b, _) => contains_generic_brackets(a) || contains_generic_brackets(b),
        Expr::Is(_, scrutinee, _) => contains_generic_brackets(scrutinee),
        Expr::DotVariant(_, _, args) => args.iter().any(|a| contains_generic_brackets(&a.value)),
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => contains_generic_brackets(e),
            ClosureBody::Suite(_) => true,
        },
        Expr::Send(_, inner) => contains_generic_brackets(inner),
        Expr::Tuple(_, items) | Expr::List(_, items) => items.iter().any(contains_generic_brackets),
        Expr::ArrayRepeat(_, elem, count) => {
            contains_generic_brackets(elem) || contains_generic_brackets(count)
        }
    }
}

fn encode_char_literal(ch: char) -> String {
    match ch {
        '\'' => "'\\''".to_string(),
        '\\' => "'\\\\'".to_string(),
        '\n' => "'\\n'".to_string(),
        '\r' => "'\\r'".to_string(),
        '\t' => "'\\t'".to_string(),
        '\0' => "'\\0'".to_string(),
        c if !c.is_control() => format!("'{c}'"),
        c => format!("'\\u{{{:x}}}'", c as u32),
    }
}

fn value_to_const_arg_expr(
    v: &crate::eval::Value,
    enum_name: Option<&str>,
    mctx: &ModuleCtx,
    span: Span,
) -> Result<Expr, SemaError> {
    use crate::eval::value;
    match v {
        crate::eval::Value::Bool(b) => Ok(Expr::Bool(span, *b)),
        crate::eval::Value::Char(c) => Ok(Expr::Char(span, encode_char_literal(*c))),
        crate::eval::Value::Enum(idx, payload) if payload.is_empty() => {
            let Some(enum_name) = enum_name else {
                return Err(unimplemented_at("this const generic argument is", span));
            };
            let variant = match enum_name {
                "Option" => match *idx {
                    value::OPTION_NONE => "None".to_string(),
                    value::OPTION_SOME => "Some".to_string(),
                    _ => return Err(unimplemented_at("this const generic argument is", span)),
                },
                "Result" => match *idx {
                    value::RESULT_OK => "Ok".to_string(),
                    value::RESULT_ERR => "Err".to_string(),
                    _ => return Err(unimplemented_at("this const generic argument is", span)),
                },
                _ => {
                    if let Some(vs) = crate::sema::stdlib_enums::variant_strs(enum_name)? {
                        vs.get(*idx).map(|v| v.to_string()).ok_or_else(|| {
                            unimplemented_at("this const generic argument is", span)
                        })?
                    } else {
                        let Some(en) = mctx.enums.get(enum_name) else {
                            return Err(unimplemented_at("this const generic argument is", span));
                        };
                        let Some(dv) = en.variants.get(*idx) else {
                            return Err(unimplemented_at("this const generic argument is", span));
                        };
                        dv.name.clone()
                    }
                }
            };
            Ok(Expr::Field(
                Box::new(Expr::Name(span, enum_name.to_string())),
                span,
                variant,
            ))
        }
        other => value::as_i128(other)
            .map(|n| Expr::Int(span, n.to_string()))
            .ok_or_else(|| unimplemented_at("this const generic argument is", span)),
    }
}

fn eval_const_expr(e: &Expr, expected: Option<&Type>, mctx: &ModuleCtx) -> Result<Expr, SemaError> {
    let span = e.span();
    let mut fctx = bodies::FnCtx::new(Type::Unit, mctx.module_pools.clone());
    let typed = bodies::check_expr(e, expected, &mut fctx, mctx)?;
    let enum_name = match &typed.ty {
        Type::Named(name, _) => Some(name.clone()),
        _ => None,
    };
    let program = build_consts_program(mctx)?;
    let value =
        crate::eval::interp::eval_standalone(&program, &typed, "<generic argument>".to_string())
            .map_err(crate::eval::to_sema_error)?;
    value_to_const_arg_expr(&value, enum_name.as_deref(), mctx, span)
}

pub(crate) fn resolve_call_targs(
    targs: &[Expr],
    mctx: &ModuleCtx,
) -> Result<Vec<TypeArg>, SemaError> {
    targs
        .iter()
        .map(|e| resolve_call_type_arg(e, mctx))
        .collect()
}

fn resolve_call_type_arg(e: &Expr, mctx: &ModuleCtx) -> Result<TypeArg, SemaError> {
    match e {
        Expr::Name(_, name) => {
            if let Some(t) = bodies::scalar_type_by_name(name) {
                return Ok(TypeArg::Type(t));
            }
            if mctx.structs.contains_key(name) || mctx.enums.contains_key(name) {
                return Ok(TypeArg::Type(Type::Named(name.clone(), vec![])));
            }
            Ok(TypeArg::Const(e.clone()))
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Char(..) | Expr::Field(..) => {
            Ok(TypeArg::Const(e.clone()))
        }
        Expr::Index(base, _, args) => {
            let Expr::Name(_, name) = base.as_ref() else {
                return Err(unimplemented_at("this generic type argument is", e.span()));
            };
            if !mctx.structs.contains_key(name) && !mctx.enums.contains_key(name) {
                return Err(SemaError::at(
                    "type",
                    format!("unknown generic type `{name}`"),
                    e.span(),
                ));
            }
            Ok(TypeArg::Type(Type::Named(
                name.clone(),
                resolve_call_targs(args, mctx)?,
            )))
        }
        other => Err(unimplemented_at(
            "this generic argument shape is",
            other.span(),
        )),
    }
}

fn check_arity(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    name: &str,
    call_span: Span,
) -> Result<(), SemaError> {
    if generics.len() != args.len() {
        return Err(SemaError::at(
            "type",
            format!(
                "`{name}` expects {} generic argument(s), found {}",
                generics.len(),
                args.len()
            ),
            call_span,
        ));
    }
    Ok(())
}

fn build_subst(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Subst, SemaError> {
    let mut subst = Subst::default();
    for (g, a) in generics.iter().zip(args.iter()) {
        match (&g.kind, a) {
            (DeclGenericKind::Type, TypeArg::Type(t)) => {
                subst.types.insert(g.name.clone(), t.clone());
            }
            (DeclGenericKind::Const(cty), TypeArg::Const(e))
            | (DeclGenericKind::Const(cty), TypeArg::Bound(e)) => {
                let v = eval_const_expr(e, Some(cty), mctx)?;
                subst.consts.insert(g.name.clone(), v);
            }
            (DeclGenericKind::Type, _) => {
                return Err(SemaError::at(
                    "type",
                    format!("generic parameter `{}` requires a type argument", g.name),
                    call_span,
                ));
            }
            (DeclGenericKind::Const(_), _) => {
                return Err(SemaError::at(
                    "type",
                    format!("generic parameter `{}` requires a const argument", g.name),
                    call_span,
                ));
            }
        }
    }
    Ok(subst)
}

fn subst_decl_field(f: &DeclField, subst: &Subst) -> DeclField {
    DeclField {
        name: f.name.clone(),
        ty: subst_type(&f.ty, subst),
        is_pub: f.is_pub,
    }
}

fn subst_decl_param(p: &DeclParam, subst: &Subst) -> DeclParam {
    DeclParam {
        mode: p.mode,
        name: p.name.clone(),
        ty: subst_type(&p.ty, subst),
    }
}

fn subst_decl_fn_member(f: &DeclFn, subst: &Subst) -> DeclFn {
    DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        is_task: f.is_task,
        generics: f.generics.clone(),
        receiver: f.receiver.clone(),
        params: f
            .params
            .iter()
            .map(|p| subst_decl_param(p, subst))
            .collect(),
        ret: subst_type(&f.ret, subst),
    }
}

fn subst_decl_fn_direct(f: &DeclFn, subst: &Subst) -> DeclFn {
    DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        is_task: f.is_task,
        generics: Vec::new(),
        receiver: f.receiver.clone(),
        params: f
            .params
            .iter()
            .map(|p| subst_decl_param(p, subst))
            .collect(),
        ret: subst_type(&f.ret, subst),
    }
}

fn subst_decl_member(m: &DeclMember, subst: &Subst) -> DeclMember {
    match m {
        DeclMember::Field(f) => DeclMember::Field(subst_decl_field(f, subst)),
        DeclMember::Fn(f) => DeclMember::Fn(subst_decl_fn_member(f, subst)),
        DeclMember::Init(f) => DeclMember::Init(subst_decl_fn_member(f, subst)),
        DeclMember::Pool(p) => DeclMember::Pool(p.clone()),
    }
}

fn reclassify(
    is_resource_fiat: bool,
    component_types: &[(Type, Span)],
    mctx: &ModuleCtx,
) -> Classification {
    let resource = is_resource_fiat
        || component_types
            .iter()
            .any(|(t, _)| bodies::is_resource_type(t, mctx));
    if resource {
        Classification::Resource
    } else {
        Classification::Data
    }
}

fn subst_decl_struct(d: &DeclStruct, subst: &Subst, mctx: &ModuleCtx) -> DeclStruct {
    let members: Vec<DeclMember> = d
        .members
        .iter()
        .map(|m| subst_decl_member(m, subst))
        .collect();
    let component_types: Vec<(Type, Span)> = d
        .component_types
        .iter()
        .map(|(t, sp)| (subst_type(t, subst), *sp))
        .collect();
    let classification = reclassify(d.is_resource_fiat, &component_types, mctx);
    DeclStruct {
        name: d.name.clone(),
        generics: Vec::new(),
        deriving: d.deriving.clone(),
        classification,
        members,
        is_resource_fiat: d.is_resource_fiat,
        is_actor: d.is_actor,
        is_driver: d.is_driver,
        layout_kind: d.layout_kind,
        component_types,
        span: d.span,
        is_manual_resource: d.is_manual_resource,
        classes: d.classes,
        classes_assigned: d.classes_assigned,
    }
}

fn subst_variant_payload(p: &DeclVariantPayload, subst: &Subst) -> DeclVariantPayload {
    match p {
        DeclVariantPayload::None => DeclVariantPayload::None,
        DeclVariantPayload::Tuple(ts) => {
            DeclVariantPayload::Tuple(ts.iter().map(|t| subst_type(t, subst)).collect())
        }
        DeclVariantPayload::Named(fs) => DeclVariantPayload::Named(
            fs.iter()
                .map(|(n, t)| (n.clone(), subst_type(t, subst)))
                .collect(),
        ),
    }
}

fn subst_decl_enum(d: &DeclEnum, subst: &Subst, mctx: &ModuleCtx) -> DeclEnum {
    let variants: Vec<DeclVariant> = d
        .variants
        .iter()
        .map(|v| DeclVariant {
            name: v.name.clone(),
            payload: subst_variant_payload(&v.payload, subst),
        })
        .collect();
    let members: Vec<DeclMember> = d
        .members
        .iter()
        .map(|m| subst_decl_member(m, subst))
        .collect();
    let component_types: Vec<(Type, Span)> = d
        .component_types
        .iter()
        .map(|(t, sp)| (subst_type(t, subst), *sp))
        .collect();
    let classification = reclassify(false, &component_types, mctx);
    DeclEnum {
        name: d.name.clone(),
        generics: Vec::new(),
        deriving: d.deriving.clone(),
        classification,
        variants,
        members,
        component_types,
        span: d.span,
        classes: d.classes,
        classes_assigned: d.classes_assigned,
    }
}

pub(crate) fn instantiate_struct(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<StructInfo, SemaError> {
    let Some(orig) = mctx.structs.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.decl.generics, args, name, call_span)?;
    let subst = build_subst(&orig.decl.generics, args, mctx, call_span)?;
    let const_subst: BTreeMap<String, Expr> = subst.consts.clone();
    let expanded = crate::sema::specialize::expand_deferred_members(
        &orig.ast_members,
        &orig.deferred_comptime_members,
        &const_subst,
        mctx,
    )?;
    let mut expanded: Vec<Member> = expanded
        .iter()
        .map(|m| subst_member_ast(m, &subst))
        .collect();
    let canonical_renderer = name == "Renderer"
        && mctx
            .type_decl_module
            .get(name)
            .zip(mctx.type_decl_name.get(name))
            .is_some_and(|(module, declaration)| {
                declaration == "Renderer" && matches!(module.as_str(), "render" | "core.render")
            });
    if canonical_renderer && let [TypeArg::Type(parameter_type)] = args {
        // The generated parameter validation and snapshot bodies are proof
        // boundaries: a silently skipped splice would leave the stdlib stub
        // in place — no `@range` validation and an all-zero snapshot — and
        // every parameterized frame would render against proofs that do not
        // apply. A missing target function is therefore a hard compiler
        // error, not a no-op, so a stdlib rename cannot fail open.
        let validation = renderer_parameter_validation(parameter_type, mctx)?;
        if !validation.is_empty() {
            let Some(Member::Fn(validate)) = expanded
                .iter_mut()
                .find(|member| matches!(member, Member::Fn(function) if function.name == "__validate_frame"))
            else {
                return Err(SemaError::at(
                    "pixels P5",
                    "canonical Renderer is missing `__validate_frame`: generated parameter \
                     validation has no splice target"
                        .to_string(),
                    call_span,
                ));
            };
            let Some(body) = validate.body.as_mut() else {
                return Err(SemaError::at(
                    "pixels P5",
                    "canonical Renderer `__validate_frame` has no body to splice generated \
                     parameter validation into"
                        .to_string(),
                    call_span,
                ));
            };
            body.splice(0..0, validation);
        }
        let snapshot = renderer_snapshot_body(parameter_type, mctx)?;
        let Some(Member::Fn(function)) = expanded.iter_mut().find(
            |member| matches!(member, Member::Fn(function) if function.name == "__snapshot_frame"),
        ) else {
            return Err(SemaError::at(
                "pixels P5",
                "canonical Renderer is missing `__snapshot_frame`: the generated parameter \
                 snapshot has no replacement target"
                    .to_string(),
                call_span,
            ));
        };
        let Some(body) = function.body.as_mut() else {
            return Err(SemaError::at(
                "pixels P5",
                "canonical Renderer `__snapshot_frame` has no body to replace with the \
                 generated parameter snapshot"
                    .to_string(),
                call_span,
            ));
        };
        *body = snapshot;
    }
    let decl = if orig.deferred_comptime_members.is_empty()
        && !orig
            .ast_members
            .iter()
            .any(|m| member_has_deferred_comptime_stmt(m))
    {
        subst_decl_struct(&orig.decl, &subst, mctx)
    } else {
        let mut decl = types::declare_struct_members_for_instantiation(
            name, &expanded, &orig.decl, mctx, call_span,
        )?;
        decl = subst_decl_struct(&decl, &subst, mctx);
        decl
    };
    bodies::enqueue_instantiation(mctx, InstKind::Struct, name, args, call_span)?;
    Ok(StructInfo {
        decl,
        ast_members: std::sync::Arc::new(expanded),
        deferred_comptime_members: Vec::new(),
    })
}

fn member_has_deferred_comptime_stmt(m: &Member) -> bool {
    match m {
        Member::Fn(f) => f
            .body
            .as_ref()
            .is_some_and(|b| stmts_have_deferred_comptime(b)),
        Member::Init(i) => stmts_have_deferred_comptime(&i.body),
        Member::ComptimeIf(_) => true,
        Member::Field(_) | Member::Pool(_) => false,
    }
}

fn stmts_have_deferred_comptime(stmts: &[crate::syntax::ast::Stmt]) -> bool {
    use crate::syntax::ast::Stmt;
    stmts.iter().any(|s| match s {
        Stmt::ComptimeIf(_) => true,
        Stmt::If(i) => {
            stmts_have_deferred_comptime(&i.then_branch)
                || i.elifs
                    .iter()
                    .any(|e| stmts_have_deferred_comptime(&e.body))
                || i.else_branch
                    .as_ref()
                    .is_some_and(|b| stmts_have_deferred_comptime(b))
        }
        Stmt::Match(m) => m.arms.iter().any(|a| stmts_have_deferred_comptime(&a.body)),
        Stmt::For(f) => stmts_have_deferred_comptime(&f.body),
        Stmt::While(w) => stmts_have_deferred_comptime(&w.body),
        Stmt::Defer(d) => match &d.body {
            crate::syntax::ast::DeferBody::Suite(s) => stmts_have_deferred_comptime(s),
            crate::syntax::ast::DeferBody::Expr(_) => false,
        },
        Stmt::With(w) => stmts_have_deferred_comptime(&w.body),
        _ => false,
    })
}

pub(crate) fn instantiate_enum(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<DeclEnum, SemaError> {
    let Some(orig) = mctx.enums.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.generics, args, name, call_span)?;
    let subst = build_subst(&orig.generics, args, mctx, call_span)?;
    let decl = subst_decl_enum(orig, &subst, mctx);
    bodies::enqueue_instantiation(mctx, InstKind::Enum, name, args, call_span)?;
    Ok(decl)
}

pub(crate) fn instantiate_fn(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<FnInfo, SemaError> {
    let Some(orig) = mctx.fns.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown function `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.decl.generics, args, name, call_span)?;
    let subst = build_subst(&orig.decl.generics, args, mctx, call_span)?;
    let decl = subst_decl_fn_direct(&orig.decl, &subst);
    let mut ast = (*orig.ast).clone();
    ast.generics = Vec::new();
    if let Some(body) = ast.body.as_mut() {
        *body = body.iter().map(|s| subst_stmt(s, &subst)).collect();
    }
    bodies::enqueue_instantiation(mctx, InstKind::Fn, name, args, call_span)?;
    Ok(FnInfo {
        ast: std::sync::Arc::new(ast),
        decl,
    })
}

pub(crate) fn instantiate_method(
    mctx: &ModuleCtx,
    receiver: &Type,
    method: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<(ast::FnItem, DeclFn), SemaError> {
    let Type::Named(type_name, type_args) = receiver else {
        return Err(SemaError::at(
            "type",
            format!(
                "method `{method}` called on non-nominal type `{}`",
                types::render_type(receiver)
            ),
            call_span,
        ));
    };
    let (ast_orig, decl_orig) = lookup_method_decl(mctx, type_name, type_args, method, call_span)?;
    check_arity(&decl_orig.generics, args, method, call_span)?;
    let subst = build_subst(&decl_orig.generics, args, mctx, call_span)?;
    let decl = subst_decl_fn_direct(&decl_orig, &subst);
    let mut ast = ast_orig;
    ast.generics = Vec::new();
    if let Some(body) = ast.body.as_mut() {
        *body = body.iter().map(|s| subst_stmt(s, &subst)).collect();
    }
    bodies::enqueue_method_instantiation(mctx, receiver, method, args, call_span)?;
    Ok((ast, decl))
}

fn lookup_method_decl(
    mctx: &ModuleCtx,
    type_name: &str,
    type_args: &[TypeArg],
    method: &str,
    call_span: Span,
) -> Result<(ast::FnItem, DeclFn), SemaError> {
    if let Some(s) = mctx.structs.get(type_name) {
        let info = if type_args.is_empty() {
            s.clone()
        } else {
            instantiate_struct(mctx, type_name, type_args, call_span)?
        };
        if let Some((f, d)) = info.method(method).or_else(|| info.assoc_fn(method)) {
            return Ok((f.clone(), d.clone()));
        }
        return Err(SemaError::at(
            "type",
            format!("type `{type_name}` has no method `{method}`"),
            call_span,
        ));
    }
    if let Some(e) = mctx.enums.get(type_name) {
        if !type_args.is_empty() {
            return Err(unimplemented_at("generic instantiation is", call_span));
        }
        if let Some((f, d)) = e.method(method).or_else(|| e.assoc_fn(method)) {
            return Ok((f.clone(), d.clone()));
        }
        return Err(SemaError::at(
            "type",
            format!("type `{type_name}` has no method `{method}`"),
            call_span,
        ));
    }
    Err(SemaError::at(
        "type",
        format!("unknown type `{type_name}`"),
        call_span,
    ))
}

pub(crate) fn infer_fn_targs(
    fi: &FnInfo,
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    infer_generic_targs(
        &fi.decl.name,
        &fi.decl.generics,
        &fi.decl.params,
        args,
        fctx,
        mctx,
        call_span,
    )
}

pub(crate) fn infer_method_targs(
    method_name: &str,
    decl: &DeclFn,
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    infer_generic_targs(
        method_name,
        &decl.generics,
        &decl.params,
        args,
        fctx,
        mctx,
        call_span,
    )
}

fn infer_generic_targs(
    display_name: &str,
    generics: &[DeclGenericParam],
    params: &[DeclParam],
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    let bound = bind_args_positionally(params, args);
    let mut inferred: BTreeMap<String, Type> = BTreeMap::new();
    for (i, p) in params.iter().enumerate() {
        let Some(arg_expr) = bound[i] else {
            continue;
        };
        match &p.ty {
            Type::Generic(gname) => {
                let synthesized = bodies::check_expr(arg_expr, None, fctx, mctx)?.ty;
                record_inferred(&mut inferred, gname, synthesized, display_name, call_span)?;
            }
            Type::Fn(fparams, fret) => {
                if let Type::Generic(gname) = fret.as_ref() {
                    if let Some(synthesized) =
                        infer_fn_arg_return(arg_expr, fparams, fctx, mctx, call_span)?
                    {
                        record_inferred(
                            &mut inferred,
                            gname,
                            synthesized,
                            display_name,
                            call_span,
                        )?;
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::with_capacity(generics.len());
    for g in generics {
        match &g.kind {
            DeclGenericKind::Type => match inferred.get(&g.name) {
                Some(t) => out.push(TypeArg::Type(t.clone())),
                None => {
                    return Err(SemaError::at(
                        "generic",
                        format!(
                            "`{display_name}` requires explicit `[Args]`: parameter `{}` cannot be inferred",
                            g.name
                        ),
                        call_span,
                    ));
                }
            },
            DeclGenericKind::Const(_) => {
                return Err(SemaError::at(
                    "generic",
                    format!(
                        "`{display_name}` requires explicit `[Args]`: const parameter `{}` cannot be inferred",
                        g.name
                    ),
                    call_span,
                ));
            }
        }
    }
    Ok(out)
}

fn record_inferred(
    inferred: &mut BTreeMap<String, Type>,
    gname: &str,
    synthesized: Type,
    display_name: &str,
    call_span: Span,
) -> Result<(), SemaError> {
    if let Some(existing) = inferred.get(gname) {
        if !bodies::types_eq(existing, &synthesized) {
            return Err(SemaError::at(
                "generic",
                format!(
                    "`{display_name}` requires explicit `[Args]`: parameter `{gname}` is both `{}` and `{}`",
                    types::render_type(existing),
                    types::render_type(&synthesized)
                ),
                call_span,
            ));
        }
    } else {
        inferred.insert(gname.to_string(), synthesized);
    }
    Ok(())
}

fn infer_fn_arg_return(
    arg_expr: &Expr,
    fparams: &[(crate::syntax::ast::AccessMode, Type)],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Option<Type>, SemaError> {
    match arg_expr {
        Expr::Closure(c) => {
            if c.params.len() != fparams.len() {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "expected {} arguments, found {}",
                        fparams.len(),
                        c.params.len()
                    ),
                    c.span,
                ));
            }
            fctx.push_scope();
            for (cp, (_mode, ety)) in c.params.iter().zip(fparams.iter()) {
                let pty = match &cp.ty {
                    Some(t) => {
                        let resolved = mctx.resolve_type(t, &fctx.local_pools)?;
                        if !bodies::types_eq(&resolved, ety) {
                            fctx.pop_scope();
                            return Err(SemaError::at(
                                "type",
                                format!(
                                    "closure parameter `{}` expects `{}`, found `{}`",
                                    cp.name,
                                    types::render_type(ety),
                                    types::render_type(&resolved)
                                ),
                                cp.span,
                            ));
                        }
                        resolved
                    }
                    None => ety.clone(),
                };
                fctx.insert_local(cp.name.clone(), pty);
            }
            let result = match &c.body {
                ClosureBody::Expr(e) => {
                    bodies::check_expr(e, None, fctx, mctx).map(|te| Some(te.ty))
                }
                ClosureBody::Suite(stmts) => Ok(suite_inferred_return(stmts)),
            };
            fctx.pop_scope();
            result
        }
        Expr::Name(span, name) => {
            if let Some(fi) = mctx.fns.get(name) {
                if !fi.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", *span));
                }
                return Ok(Some(fi.decl.ret.clone()));
            }
            Err(SemaError::at(
                "generic",
                format!("cannot infer return type of `fn(...) -> R` argument from `{name}`"),
                call_span,
            ))
        }
        Expr::Field(base, span, name) => {
            if let Expr::Name(_, bname) = base.as_ref() {
                if fctx.lookup_local(bname).is_none() {
                    if let Some(s) = mctx.structs.get(bname.as_str()) {
                        if let Some((_, d)) = s.assoc_fn(name).or_else(|| s.method(name)) {
                            if !d.generics.is_empty() || !s.decl.generics.is_empty() {
                                return Err(unimplemented_at("generic instantiation is", *span));
                            }
                            return Ok(Some(d.ret.clone()));
                        }
                    }
                    if let Some(e) = mctx.enums.get(bname.as_str()) {
                        if let Some((_, d)) = e.assoc_fn(name).or_else(|| e.method(name)) {
                            if !d.generics.is_empty() || !e.generics.is_empty() {
                                return Err(unimplemented_at("generic instantiation is", *span));
                            }
                            return Ok(Some(d.ret.clone()));
                        }
                    }
                }
            }
            Err(SemaError::at(
                "generic",
                "cannot infer return type of `fn(...) -> R` argument from this expression"
                    .to_string(),
                call_span,
            ))
        }
        _ => Err(SemaError::at(
            "generic",
            "cannot infer return type of `fn(...) -> R` argument from this expression".to_string(),
            call_span,
        )),
    }
}

fn suite_inferred_return(stmts: &[Stmt]) -> Option<Type> {
    fn has_valued_return(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Return(_, Some(_)) => true,
            Stmt::If(i) => {
                has_valued_return(&i.then_branch)
                    || i.elifs.iter().any(|e| has_valued_return(&e.body))
                    || i.else_branch.as_ref().is_some_and(|b| has_valued_return(b))
            }
            Stmt::Match(m) => m.arms.iter().any(|a| has_valued_return(&a.body)),
            Stmt::For(f) => has_valued_return(&f.body),
            Stmt::While(w) => has_valued_return(&w.body),
            _ => false,
        })
    }
    if has_valued_return(stmts) {
        None
    } else {
        Some(Type::Unit)
    }
}

fn bind_args_positionally<'a>(decl_params: &[DeclParam], args: &'a [Arg]) -> Vec<Option<&'a Expr>> {
    let mut bound: Vec<Option<&'a Expr>> = vec![None; decl_params.len()];
    let mut cursor = 0usize;
    for a in args {
        let idx = match &a.label {
            Some(lbl) => decl_params.iter().position(|p| &p.name == lbl),
            None => {
                while cursor < decl_params.len() && bound[cursor].is_some() {
                    cursor += 1;
                }
                let i = cursor;
                cursor += 1;
                Some(i).filter(|&i| i < decl_params.len())
            }
        };
        if let Some(idx) = idx {
            bound[idx] = Some(&a.value);
        }
    }
    bound
}

pub(crate) fn check(
    _module: &Module,
    _decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
    path: &str,
) -> Result<BTreeMap<String, TypedInstantiation>, SemaError> {
    let mut processed: BTreeSet<String> = BTreeSet::new();
    let mut typed_instantiations: BTreeMap<String, TypedInstantiation> = BTreeMap::new();
    let effects = access::infer_effects_over(mctx);
    loop {
        let next = {
            let q = mctx.generics_queue.borrow();
            q.iter()
                .find(|(k, _)| !processed.contains(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
        };
        let Some((key, entry)) = next else {
            break;
        };
        processed.insert(key.clone());
        *mctx.current_chain.borrow_mut() = entry.chain.clone();
        let result = check_one_instantiation(mctx, &entry, &effects);
        *mctx.current_chain.borrow_mut() = Vec::new();
        match result {
            Ok(typed_inst) => {
                typed_instantiations.insert(key, typed_inst);
            }
            Err(e) => return Err(finalize_diagnostic(e, &entry, mctx, path)),
        }
    }
    Ok(typed_instantiations)
}

fn check_one_instantiation(
    mctx: &ModuleCtx,
    entry: &QueuedInstantiation,
    effects: &access::EffectMap,
) -> Result<TypedInstantiation, SemaError> {
    let call_span = *entry
        .chain
        .last()
        .expect("a queued instantiation's chain always has at least its own triggering call");
    let home = instantiation_visibility_home(mctx, entry);
    *mctx.visibility_home.borrow_mut() = Some(home);
    let result = check_one_instantiation_inner(mctx, entry, call_span, effects);
    *mctx.visibility_home.borrow_mut() = None;
    result
}

fn instantiation_visibility_home(mctx: &ModuleCtx, entry: &QueuedInstantiation) -> String {
    match entry.kind {
        InstKind::Fn => mctx
            .fn_decl_module
            .get(&entry.name)
            .cloned()
            .unwrap_or_else(|| mctx.module_path.clone()),
        InstKind::Struct => mctx
            .struct_decl_module
            .get(&entry.name)
            .cloned()
            .unwrap_or_else(|| mctx.module_path.clone()),
        InstKind::Method => {
            let receiver = entry
                .receiver
                .as_ref()
                .expect("InstKind::Method always carries a receiver type");
            if let Type::Named(type_name, _) = receiver {
                mctx.struct_decl_module
                    .get(type_name)
                    .cloned()
                    .unwrap_or_else(|| mctx.module_path.clone())
            } else {
                mctx.module_path.clone()
            }
        }
        InstKind::Enum => mctx.module_path.clone(),
    }
}

fn check_one_instantiation_inner(
    mctx: &ModuleCtx,
    entry: &QueuedInstantiation,
    call_span: Span,
    effects: &access::EffectMap,
) -> Result<TypedInstantiation, SemaError> {
    match entry.kind {
        InstKind::Fn => {
            let fi = instantiate_fn(mctx, &entry.name, &entry.args, call_span)?;
            let mut tf = bodies::check_top_fn(&fi.ast, &fi.decl, mctx)?
                .expect("an instantiated fn is always concrete, never itself generic");
            let empty_effects = access::EffectMap::new();
            access::check_typed_fn(&mut tf, mctx, &empty_effects)?;
            flow::check_typed_fn(&tf, mctx, &empty_effects)?;
            matches::check_fn(&tf, mctx)?;
            Ok(TypedInstantiation::Fn(tf))
        }
        InstKind::Method => {
            let receiver = entry
                .receiver
                .as_ref()
                .expect("InstKind::Method always carries a receiver type");
            let (ast, decl) =
                instantiate_method(mctx, receiver, &entry.name, &entry.args, call_span)?;
            let mini = method_instantiation_struct_info(mctx, receiver, &ast, &decl, call_span)?;
            let mut ts = bodies::check_struct_members(&mini, receiver.clone(), mctx)?;
            access::check_typed_struct(&mut ts, mctx, &effects)?;
            flow::check_typed_struct(&ts, mctx, &effects)?;
            matches::check_struct(&ts, mctx)?;
            let tf = ts
                .methods
                .get(&entry.name)
                .or_else(|| ts.assoc_fns.get(&entry.name))
                .cloned()
                .expect("instantiated method was just checked into the mini struct");
            Ok(TypedInstantiation::Fn(tf))
        }
        InstKind::Struct => {
            let si = instantiate_struct(mctx, &entry.name, &entry.args, call_span)?;
            let self_ty = Type::Named(entry.name.clone(), entry.args.clone());
            let mut ts = bodies::check_struct_members(&si, self_ty.clone(), mctx)?;
            access::check_typed_struct(&mut ts, mctx, &effects)?;
            flow::check_typed_struct(&ts, mctx, &effects)?;
            matches::check_struct(&ts, mctx)?;
            Ok(TypedInstantiation::Struct(ts))
        }
        InstKind::Enum => {
            let enumeration = instantiate_enum(mctx, &entry.name, &entry.args, call_span)?;
            let variant_payload_types = enumeration
                .variants
                .iter()
                .map(|variant| match &variant.payload {
                    DeclVariantPayload::None => Vec::new(),
                    DeclVariantPayload::Tuple(types) => types.clone(),
                    DeclVariantPayload::Named(fields) => {
                        fields.iter().map(|(_, ty)| ty.clone()).collect()
                    }
                })
                .collect();
            Ok(TypedInstantiation::Enum(TypedEnum {
                variants: enumeration
                    .variants
                    .iter()
                    .map(|variant| variant.name.clone())
                    .collect(),
                variant_payload_types,
                generic_type_params: Vec::new(),
                methods: BTreeMap::new(),
                assoc_fns: BTreeMap::new(),
            }))
        }
    }
}

fn method_instantiation_struct_info(
    mctx: &ModuleCtx,
    receiver: &Type,
    ast: &ast::FnItem,
    decl: &DeclFn,
    call_span: Span,
) -> Result<StructInfo, SemaError> {
    let Type::Named(type_name, type_args) = receiver else {
        return Err(SemaError::at(
            "type",
            format!(
                "method `{}` called on non-nominal type `{}`",
                decl.name,
                types::render_type(receiver)
            ),
            call_span,
        ));
    };
    let mut base = if let Some(s) = mctx.structs.get(type_name.as_str()) {
        if type_args.is_empty() {
            s.clone()
        } else {
            instantiate_struct(mctx, type_name, type_args, call_span)?
        }
    } else if let Some(e) = mctx.enums.get(type_name.as_str()) {
        return Ok(StructInfo {
            decl: DeclStruct {
                name: type_name.clone(),
                generics: Vec::new(),
                deriving: e.deriving.clone(),
                classification: e.classification,
                members: vec![DeclMember::Fn(decl.clone())],
                is_resource_fiat: false,
                is_actor: false,
                is_driver: false,
                layout_kind: None,
                component_types: Vec::new(),
                span: e.span,
                is_manual_resource: false,
                classes: e.classes,
                classes_assigned: e.classes_assigned,
            },
            ast_members: std::sync::Arc::new(vec![Member::Fn(ast.clone())]),
            deferred_comptime_members: Vec::new(),
        });
    } else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{type_name}`"),
            call_span,
        ));
    };
    base.decl.members = vec![DeclMember::Fn(decl.clone())];
    base.ast_members = std::sync::Arc::new(vec![Member::Fn(ast.clone())]);
    base.deferred_comptime_members = Vec::new();
    Ok(base)
}

fn finalize_diagnostic(
    e: SemaError,
    entry: &QueuedInstantiation,
    mctx: &ModuleCtx,
    path: &str,
) -> SemaError {
    if e.category == "type" && e.extra_lines.is_empty() {
        if let Some((type_name, method_name)) = e.missing_method.clone() {
            if let Some((call_expr, ret_ty)) =
                find_requirement(mctx, entry, &type_name, &method_name)
            {
                let sig = format!(
                    "{type_name}.{method_name}(read self) -> {}",
                    types::render_type(&ret_ty)
                );
                let display = display_inst_name(entry);
                let mut extra_lines = vec![format!(
                    "  required by `{}` at {path}:{}",
                    printer::print_expr_bare(call_expr),
                    call_expr.span().line
                )];
                for span in entry.chain.iter().rev() {
                    extra_lines.push(format!("  instantiated at {path}:{}", span.line));
                }
                return SemaError {
                    category: "generic",
                    message: format!("`{display}` requires `{sig}`"),
                    line: 0,
                    col: 0,
                    extra_lines,
                    omit_location: true,
                    missing_method: None,
                };
            }
        }
    }
    let mut e = e;
    for span in entry.chain.iter().rev() {
        e.extra_lines
            .push(format!("  instantiated at {path}:{}", span.line));
    }
    e
}

fn find_requirement<'a>(
    mctx: &'a ModuleCtx,
    entry: &QueuedInstantiation,
    type_name: &str,
    method_name: &str,
) -> Option<(&'a Expr, Type)> {
    match entry.kind {
        InstKind::Fn => {
            let fi = mctx.fns.get(&entry.name)?;
            find_requirement_in(
                &fi.decl.generics,
                &entry.args,
                &fi.decl.params,
                fi.ast.body.as_ref()?,
                &fi.decl.ret,
                type_name,
                method_name,
            )
        }
        InstKind::Method => {
            let receiver = entry.receiver.as_ref()?;
            let Type::Named(recv_name, recv_args) = receiver else {
                return None;
            };
            let (ast, decl) = if let Some(s) = mctx.structs.get(recv_name.as_str()) {
                if !recv_args.is_empty() {
                    let (f, d) = s.method(&entry.name).or_else(|| s.assoc_fn(&entry.name))?;
                    (f, d)
                } else {
                    let (f, d) = s.method(&entry.name).or_else(|| s.assoc_fn(&entry.name))?;
                    (f, d)
                }
            } else if let Some(e) = mctx.enums.get(recv_name.as_str()) {
                let (f, d) = e.method(&entry.name).or_else(|| e.assoc_fn(&entry.name))?;
                (f, d)
            } else {
                return None;
            };
            find_requirement_in(
                &decl.generics,
                &entry.args,
                &decl.params,
                ast.body.as_ref()?,
                &decl.ret,
                type_name,
                method_name,
            )
        }
        InstKind::Struct | InstKind::Enum => None,
    }
}

fn find_requirement_in<'a>(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    params: &[DeclParam],
    body: &'a [Stmt],
    ret: &Type,
    type_name: &str,
    method_name: &str,
) -> Option<(&'a Expr, Type)> {
    let target_param = generics
        .iter()
        .zip(args.iter())
        .find_map(|(g, a)| match (&g.kind, a) {
            (DeclGenericKind::Type, TypeArg::Type(Type::Named(n, targs)))
                if n == type_name && targs.is_empty() =>
            {
                Some(g.name.clone())
            }
            _ => None,
        })?;
    let mut param_types = BTreeMap::new();
    for p in params {
        param_types.insert(p.name.clone(), p.ty.clone());
    }
    let (call_expr, found_method) = infer_requirement_call(body, &target_param, &param_types)?;
    if found_method != method_name {
        return None;
    }
    Some((call_expr, ret.clone()))
}

fn infer_requirement_call<'a>(
    body: &'a [Stmt],
    generic_param: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<(&'a Expr, String)> {
    for stmt in body {
        if let Stmt::Return(_, Some(expr)) = stmt {
            if let Some(found) = scan_return_expr(expr, generic_param, param_types) {
                return Some(found);
            }
        }
    }
    None
}

fn scan_return_expr<'a>(
    expr: &'a Expr,
    g: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<(&'a Expr, String)> {
    if let Some(method) = direct_method_call(expr, g, param_types) {
        return Some((expr, method));
    }
    if let Expr::Binary(_, op, l, r) = expr {
        if is_same_type_result_op(*op) {
            if let Some(method) = direct_method_call(l, g, param_types) {
                return Some((l, method));
            }
            if let Some(method) = direct_method_call(r, g, param_types) {
                return Some((r, method));
            }
        }
    }
    None
}

fn is_same_type_result_op(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::AddW
            | BinOp::SubW
            | BinOp::MulW
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
    )
}

fn direct_method_call(
    expr: &Expr,
    g: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<String> {
    let Expr::Call(callee, _, args) = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let Expr::Field(base, _, method) = callee.as_ref() else {
        return None;
    };
    let Expr::Name(_, base_name) = base.as_ref() else {
        return None;
    };
    match param_types.get(base_name) {
        Some(Type::Generic(pn)) if pn == g => Some(method.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{lexer, parser};

    fn build_mctx(src: &str) -> ModuleCtx {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let decl_items = types::declare(&module).expect("test source must declare");
        bodies::build_module_ctx(&module, &decl_items, &types::ImportedTypes::new())
    }

    const SRC: &str = "module examples.const_eval

const LIMIT: u64 = 4

enum Color:
    Red
    Green
    Blue

pub fn use_const() -> u64:
    return LIMIT
";

    #[test]
    fn eval_const_expr_literal_int_bool_char() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        assert_eq!(
            eval_const_expr(&Expr::Int(span, "42".to_string()), Some(&Type::U64), &mctx).unwrap(),
            Expr::Int(span, "42".to_string())
        );
        assert_eq!(
            eval_const_expr(&Expr::Bool(span, true), Some(&Type::Bool), &mctx).unwrap(),
            Expr::Bool(span, true)
        );
        assert_eq!(
            eval_const_expr(
                &Expr::Char(span, "'x'".to_string()),
                Some(&Type::Char),
                &mctx
            )
            .unwrap(),
            Expr::Char(span, "'x'".to_string())
        );
    }

    #[test]
    fn eval_const_expr_resolves_a_const_name() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let result = eval_const_expr(
            &Expr::Name(span, "LIMIT".to_string()),
            Some(&Type::U64),
            &mctx,
        );
        assert_eq!(result.unwrap(), Expr::Int(span, "4".to_string()));
    }

    #[test]
    fn eval_const_expr_fieldless_enum_variant() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let expr = Expr::Field(
            Box::new(Expr::Name(span, "Color".to_string())),
            span,
            "Red".to_string(),
        );
        let expected_ty = Type::Named("Color".to_string(), vec![]);
        let result = eval_const_expr(&expr, Some(&expected_ty), &mctx);
        assert_eq!(
            result.unwrap(),
            Expr::Field(
                Box::new(Expr::Name(span, "Color".to_string())),
                span,
                "Red".to_string()
            )
        );
    }

    #[test]
    fn eval_const_expr_unknown_const_name_fails_closed() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        assert!(
            eval_const_expr(
                &Expr::Name(span, "NOPE".to_string()),
                Some(&Type::U64),
                &mctx
            )
            .is_err()
        );
    }

    #[test]
    fn eval_const_expr_evaluates_arithmetic() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let expr = Expr::Binary(
            span,
            BinOp::Add,
            Box::new(Expr::Int(span, "1".to_string())),
            Box::new(Expr::Int(span, "1".to_string())),
        );
        assert_eq!(
            eval_const_expr(&expr, Some(&Type::I64), &mctx).unwrap(),
            Expr::Int(span, "2".to_string())
        );
    }
}
