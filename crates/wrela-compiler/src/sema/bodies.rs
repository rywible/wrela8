//! Statement/expression typing (plans/M2.md item C): assignment
//! introduction/reassignment, `if`/`while` condition typing, `for`
//! typing, operator desugar (02-language.md §7.4, §8.2, 05-library.md
//! §8), call checking (arity, labels), enum literals and leading-dot
//! inference, pattern typing, `is`, closures as structural `fn` types,
//! `?`, `assert`, `defer`. Also where the fail-closed set (decision 7)
//! beyond imports lands: `comptime if`/`comptime assert`, f-strings,
//! `await`/`send`/`with` (group/pool), `@image` bodies.
//!
//! Shape (decision 4): no unification, no constraint solver — every
//! expression is either checked against an expected type the grammar
//! already supplies (`check_expr`), or synthesized on its own
//! (`synth_expr`, called by `check_expr`, which then gates the result
//! against `expected` when one was given). Everything clones freely
//! (`Type`/`DeclFn`/AST nodes all derive `Clone`): `ModuleCtx` below owns
//! plain copies of every declared item's ast + resolved-type pair
//! instead of borrowing, so no lifetime threads through the whole file.
//!
//! Generic declarations (item H's territory) are not type-checked here:
//! a generic struct/enum's members, and a generic fn/method's body, are
//! skipped entirely (no error — just not visited); a *use* of a generic
//! type/fn from a non-generic body that needs anything beyond structural
//! equality (field/method/variant lookup, a call) fails closed via
//! `unimplemented_at("generic instantiation is", ...)`.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::types::{self, Classification, DeclMember, DeclParam, DeclVariantPayload, Type};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssertStmt, AssignOp, AssignStmt, BinOp, ClosureBody, ClosureExpr,
    ComptimeIfStmt, DeferBody, DeferStmt, Expr, ForStmt, IfStmt, Item, MatchStmt, Member, Module,
    Pattern, Span, Stmt, UnaryOp, WhileStmt, WithStmt,
};

// --- module-wide lookup context ------------------------------------------

/// One struct's declared shape, for body typing: the resolved
/// declaration (`types::declare`'s output) plus a parallel, owned copy of
/// its ast members (same order, `comptime if` members already filtered
/// out — mirrors exactly what `types::declare_struct` iterated) so field
/// defaults, method/init bodies, param defaults, and per-member generics
/// are all available without re-walking the module.
// `pub(crate)` throughout `StructInfo`/`FnInfo`/`ModuleCtx` (plans/M2.md
// item D, decision 10's minimal-footprint rule): access.rs re-walks
// bodies with the declared signatures exactly like this pass does, so it
// reuses this same lookup context wholesale (`build_module_ctx`) rather
// than duplicating struct/enum/fn table construction — nothing here is
// restructured, only exposed.
pub(crate) struct StructInfo {
    pub(crate) decl: types::DeclStruct,
    pub(crate) ast_members: Vec<Member>,
}

impl StructInfo {
    pub(crate) fn members(&self) -> impl Iterator<Item = (&Member, &DeclMember)> {
        self.ast_members.iter().zip(self.decl.members.iter())
    }

    pub(crate) fn field_ty(&self, name: &str) -> Option<Type> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Field(f), DeclMember::Field(d)) if f.name == name => Some(d.ty.clone()),
            _ => None,
        })
    }

    pub(crate) fn has_member_named(&self, name: &str) -> bool {
        self.ast_members.iter().any(|m| match m {
            Member::Fn(f) => f.name == name,
            Member::Field(f) => f.name == name,
            _ => false,
        })
    }

    pub(crate) fn assoc_fn(&self, name: &str) -> Option<(&ast::FnItem, &types::DeclFn)> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Fn(f), DeclMember::Fn(d)) if f.name == name && f.receiver.is_none() => {
                Some((f, d))
            }
            _ => None,
        })
    }

    pub(crate) fn method(&self, name: &str) -> Option<(&ast::FnItem, &types::DeclFn)> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Fn(f), DeclMember::Fn(d)) if f.name == name && f.receiver.is_some() => {
                Some((f, d))
            }
            _ => None,
        })
    }

    pub(crate) fn init(&self) -> Option<(&ast::InitItem, &types::DeclFn)> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Init(i), DeclMember::Init(d)) => Some((i, d)),
            _ => None,
        })
    }
}

/// One top-level fn's ast (params/defaults/generics/attrs/body) plus its
/// resolved declaration.
pub(crate) struct FnInfo {
    pub(crate) ast: ast::FnItem,
    pub(crate) decl: types::DeclFn,
}

/// Everything body-typing needs to resolve names beyond the current
/// function: struct/enum/fn/const declarations, generic arity (for
/// annotation resolution and the generic-instantiation fail-closed
/// check), and module-scope pool names. Built once per `check` call from
/// `module` + `declare`'s already-resolved `decl_items`; nothing here
/// borrows either, so no lifetime parameter is needed anywhere in this
/// file (decision 4: clone freely).
pub(crate) struct ModuleCtx {
    pub(crate) shapes: BTreeMap<String, usize>,
    pub(crate) module_pools: BTreeSet<String>,
    pub(crate) structs: BTreeMap<String, StructInfo>,
    pub(crate) enums: BTreeMap<String, types::DeclEnum>,
    pub(crate) fns: BTreeMap<String, FnInfo>,
    pub(crate) consts: BTreeMap<String, Type>,
}

impl ModuleCtx {
    /// Resolves an ast type exactly like `types::declare` did (reusing
    /// its own `resolve_type`), with no generics in scope — every body
    /// this pass actually checks lives inside a non-generic declaration
    /// (item H's job otherwise), so a local annotation, closure param
    /// annotation, etc. can never legally name a generic parameter here.
    pub(crate) fn resolve_type(
        &self,
        ty: &ast::Type,
        local_pools: &BTreeSet<String>,
    ) -> Result<Type, SemaError> {
        types::resolve_type(
            ty,
            &self.shapes,
            &self.module_pools,
            local_pools,
            &BTreeMap::new(),
            false,
        )
    }
}

pub(crate) fn build_module_ctx(module: &Module, decl_items: &[types::DeclItem]) -> ModuleCtx {
    let mut shapes = BTreeMap::new();
    let mut module_pools = BTreeSet::new();
    let mut structs = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut consts = BTreeMap::new();

    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();

    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Struct(s), types::DeclItem::Struct(d)) => {
                shapes.insert(s.name.clone(), s.generics.len());
                let ast_members: Vec<Member> = s
                    .members
                    .iter()
                    .filter(|m| !matches!(m, Member::ComptimeIf(_)))
                    .cloned()
                    .collect();
                structs.insert(
                    s.name.clone(),
                    StructInfo {
                        decl: d.clone(),
                        ast_members,
                    },
                );
            }
            (Item::Enum(e), types::DeclItem::Enum(d)) => {
                shapes.insert(e.name.clone(), e.generics.len());
                enums.insert(e.name.clone(), d.clone());
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => {
                fns.insert(
                    f.name.clone(),
                    FnInfo {
                        ast: f.clone(),
                        decl: d.clone(),
                    },
                );
            }
            (Item::Const(c), types::DeclItem::Const(d)) => {
                consts.insert(c.name.clone(), d.ty.clone());
            }
            (Item::Pool(p), types::DeclItem::Pool(_)) => {
                module_pools.insert(p.name.clone());
            }
            _ => unreachable!("declare()'s items must pair 1:1 with the filtered ast items"),
        }
    }

    ModuleCtx {
        shapes,
        module_pools,
        structs,
        enums,
        fns,
        consts,
    }
}

// --- per-body checking context -------------------------------------------

/// One function/method/init/closure body's typing state: the current
/// return type (for `return`/`?`), a local-variable scope stack (only a
/// closure pushes a new one — mirrors `symbols::Resolver`'s scope model
/// exactly, decision 3's name-resolution shape reused for types), and
/// the pool names visible by bare name inside `own[P]` annotations here
/// (a struct's own `pool` members, when checking one of its
/// methods/init; otherwise just the module's).
struct FnCtx {
    ret_ty: Type,
    locals: Vec<BTreeMap<String, Type>>,
    local_pools: BTreeSet<String>,
}

impl FnCtx {
    fn new(ret_ty: Type, local_pools: BTreeSet<String>) -> FnCtx {
        FnCtx {
            ret_ty,
            locals: vec![BTreeMap::new()],
            local_pools,
        }
    }

    fn push_scope(&mut self) {
        self.locals.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }

    /// A read-position lookup: innermost scope outward, matching
    /// `symbols::Resolver::resolve_name`'s search order.
    fn lookup_local(&self, name: &str) -> Option<Type> {
        for scope in self.locals.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    fn lookup_innermost(&self, name: &str) -> Option<Type> {
        self.locals
            .last()
            .expect("at least one scope")
            .get(name)
            .cloned()
    }

    fn insert_local(&mut self, name: String, ty: Type) {
        self.locals
            .last_mut()
            .expect("at least one scope")
            .insert(name, ty);
    }
}

/// Binds `name` to `ty` in the current (innermost) scope: a plain insert
/// if this is the first binding, an equality check if `name` is already
/// bound there — this is how a match arm's pattern binding, a `for`
/// binding, and a fresh assignment all interact with a name reused by an
/// *earlier* sibling branch in the same flat scope (name resolution
/// permits this — only a closure pushes a new scope — so typing must
/// decide what happens: reusing the name requires the same type, which
/// is a dumb, sound, non-flow-sensitive stand-in for the real arm-merge
/// analysis flow's pass (items E/F) owns).
fn bind_local(fctx: &mut FnCtx, name: &str, ty: Type, span: Span) -> Result<(), SemaError> {
    if let Some(existing) = fctx.lookup_innermost(name) {
        if !types_eq(&existing, &ty) {
            return Err(type_error(
                format!(
                    "`{name}` is already bound to type `{}` here; found `{}`",
                    types::render_type(&existing),
                    types::render_type(&ty)
                ),
                span,
            ));
        }
        Ok(())
    } else {
        fctx.insert_local(name.to_string(), ty);
        Ok(())
    }
}

// --- entry point -----------------------------------------------------------

/// The body-typing pass (plans/M2.md item C): runs after `declare`
/// (types.rs), which this needs for every signature/field/classification
/// already resolved. Fail-fast, source order, one module-wide walk: for
/// each top-level `const`/`fn`/`struct`, checks its body/bodies; `enum`
/// and `pool` items have none. A generic declaration's own body (or a
/// generic member's, inside an otherwise-concrete struct) is skipped —
/// not an error, just unchecked (item H's job).
pub fn check(module: &Module, decl_items: &[types::DeclItem]) -> Result<(), SemaError> {
    let mctx = build_module_ctx(module, decl_items);
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Const(c), types::DeclItem::Const(d)) => {
                let mut fctx = FnCtx::new(Type::Unit, mctx.module_pools.clone());
                check_expr(&c.value, Some(&d.ty), &mut fctx, &mctx)?;
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => check_top_fn(f, d, &mctx)?,
            (Item::Struct(s), types::DeclItem::Struct(d)) => check_struct_bodies(s, d, &mctx)?,
            _ => {}
        }
    }
    Ok(())
}

fn is_image_fn(f: &ast::FnItem) -> bool {
    f.attrs.iter().any(|a| a.name == "image")
}

fn check_top_fn(f: &ast::FnItem, d: &types::DeclFn, mctx: &ModuleCtx) -> Result<(), SemaError> {
    if is_image_fn(f) {
        // The whole declaration is unchecked (decision 7): the image
        // constructor's semantics (device/actor/pool wiring) are M4's.
        return Err(unimplemented_at("@image bodies are", f.span));
    }
    if !f.generics.is_empty() {
        return Ok(()); // generic body: item H's job, not checked here.
    }
    let mut fctx = FnCtx::new(d.ret.clone(), mctx.module_pools.clone());
    check_params_with_defaults(&f.params, &d.params, &mut fctx, mctx)?;
    if let Some(body) = &f.body {
        check_stmts(body, &mut fctx, mctx)?;
    }
    Ok(())
}

fn check_params_with_defaults(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    for (ap, dp) in ast_params.iter().zip(decl_params.iter()) {
        fctx.insert_local(dp.name.clone(), dp.ty.clone());
        if let Some(def) = &ap.default {
            check_expr(def, Some(&dp.ty), fctx, mctx)?;
        }
    }
    Ok(())
}

fn check_struct_bodies(
    s: &ast::StructItem,
    _d: &types::DeclStruct,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    if !s.generics.is_empty() {
        return Ok(()); // generic struct: item H's job, not checked here.
    }
    let self_ty = Type::Named(s.name.clone(), vec![]);
    let local_pools: BTreeSet<String> = s
        .members
        .iter()
        .filter_map(|m| match m {
            Member::Pool(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect();
    let info = mctx.structs.get(&s.name).expect("struct present in mctx");
    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Field(af), DeclMember::Field(df)) => {
                if let Some(def) = &af.default {
                    let mut fctx = FnCtx::new(Type::Unit, local_pools.clone());
                    fctx.insert_local("self".to_string(), self_ty.clone());
                    check_expr(def, Some(&df.ty), &mut fctx, mctx)?;
                }
            }
            (Member::Fn(f), DeclMember::Fn(fd)) => {
                if !f.generics.is_empty() {
                    continue; // generic method: item H's job.
                }
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                check_params_with_defaults(&f.params, &fd.params, &mut fctx, mctx)?;
                if let Some(body) = &f.body {
                    check_stmts(body, &mut fctx, mctx)?;
                }
            }
            (Member::Init(i), DeclMember::Init(fd)) => {
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                check_params_with_defaults(&i.params, &fd.params, &mut fctx, mctx)?;
                check_stmts(&i.body, &mut fctx, mctx)?;
            }
            _ => {}
        }
    }
    Ok(())
}

// --- statements --------------------------------------------------------

fn check_stmts(stmts: &[Stmt], fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    for s in stmts {
        check_stmt(s, fctx, mctx)?;
    }
    Ok(())
}

fn check_stmt(stmt: &Stmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match stmt {
        Stmt::Assign(a) => check_assign(a, fctx, mctx),
        Stmt::If(i) => check_if(i, fctx, mctx),
        Stmt::Match(m) => check_match(m, fctx, mctx),
        Stmt::For(f) => check_for(f, fctx, mctx),
        Stmt::While(w) => check_while(w, fctx, mctx),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => Ok(()),
        Stmt::Return(span, e) => check_return(*span, e, fctx, mctx),
        Stmt::Assert(a) => check_assert(a, fctx, mctx),
        Stmt::Defer(d) => check_defer(d, fctx, mctx),
        Stmt::With(w) => Err(unimplemented_at("`with` is", with_span(w))),
        Stmt::Send(span, _e) => Err(unimplemented_at("`send` is", *span)),
        Stmt::Expr(_span, e) => {
            check_expr(e, None, fctx, mctx)?;
            Ok(())
        }
        Stmt::ComptimeIf(c) => Err(unimplemented_at("`comptime if` is", comptime_if_span(c))),
        Stmt::ComptimeAssert(span, _, _) => Err(unimplemented_at("`comptime assert` is", *span)),
    }
}

fn with_span(w: &WithStmt) -> Span {
    w.span
}
fn comptime_if_span(c: &ComptimeIfStmt) -> Span {
    c.span
}

fn check_if(i: &IfStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    check_expr(&i.cond, Some(&Type::Bool), fctx, mctx)?;
    check_stmts(&i.then_branch, fctx, mctx)?;
    for elif in &i.elifs {
        check_expr(&elif.cond, Some(&Type::Bool), fctx, mctx)?;
        check_stmts(&elif.body, fctx, mctx)?;
    }
    if let Some(b) = &i.else_branch {
        check_stmts(b, fctx, mctx)?;
    }
    Ok(())
}

fn check_while(w: &WhileStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    check_expr(&w.cond, Some(&Type::Bool), fctx, mctx)?;
    check_stmts(&w.body, fctx, mctx)
}

fn check_match(m: &MatchStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    let scrutinee = check_expr(&m.scrutinee, None, fctx, mctx)?;
    for arm in &m.arms {
        check_pattern(&arm.pattern, &scrutinee, fctx, mctx)?;
        if let Some(g) = &arm.guard {
            check_expr(g, Some(&Type::Bool), fctx, mctx)?;
        }
        check_stmts(&arm.body, fctx, mctx)?;
    }
    Ok(())
}

fn check_return(
    span: Span,
    e: &Option<Expr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    match e {
        Some(expr) => {
            let ret_ty = fctx.ret_ty.clone();
            check_expr(expr, Some(&ret_ty), fctx, mctx)?;
            Ok(())
        }
        None => {
            if !types_eq(&fctx.ret_ty, &Type::Unit) {
                return Err(type_error(
                    format!(
                        "expected a return value of type `{}`",
                        types::render_type(&fctx.ret_ty)
                    ),
                    span,
                ));
            }
            Ok(())
        }
    }
}

fn check_assert(a: &AssertStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    check_expr(&a.cond, Some(&Type::Bool), fctx, mctx)?;
    if let Some(msg) = &a.message {
        match msg {
            Expr::Str(..) => {
                check_expr(msg, None, fctx, mctx)?;
            }
            Expr::FStr(_) => return Err(unimplemented_at("f-strings are", msg.span())),
            other => {
                return Err(type_error(
                    "assert message must be a text literal".to_string(),
                    other.span(),
                ));
            }
        }
    }
    Ok(())
}

fn check_for(f: &ForStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    let raw_iterable: &Expr = match &f.iterable {
        Expr::Unary(_, UnaryOp::Take, inner) => inner.as_ref(),
        other => other,
    };
    let elem_ty = match raw_iterable {
        Expr::Range(rspan, from, to, _incl) => {
            let fty = check_same_type_operands(from, to, fctx, mctx)?;
            if !is_integer_scalar(&fty) {
                return Err(type_error(
                    format!(
                        "range endpoints must be an integer type, found `{}`",
                        types::render_type(&fty)
                    ),
                    *rspan,
                ));
            }
            fty
        }
        other => {
            let ty = check_expr(other, None, fctx, mctx)?;
            match ty {
                Type::Array(elem, _) => *elem,
                _ => {
                    return Err(type_error(
                        format!(
                            "`for` requires a range or fixed array, found `{}`",
                            types::render_type(&ty)
                        ),
                        other.span(),
                    ));
                }
            }
        }
    };
    bind_local(fctx, &f.name, elem_ty, f.span)?;
    check_stmts(&f.body, fctx, mctx)
}

fn check_defer(d: &DeferStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    if let Some((what, span)) = scan_defer_forbidden(&d.body) {
        return Err(type_error(format!("defer body cannot {what}"), span));
    }
    match &d.body {
        DeferBody::Expr(e) => {
            check_expr(e, None, fctx, mctx)?;
            Ok(())
        }
        DeferBody::Suite(stmts) => check_stmts(stmts, fctx, mctx),
    }
}

fn check_assign(a: &AssignStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    if matches!(a.value, Expr::Closure(_)) {
        return Err(type_error("closures cannot be stored".to_string(), a.span));
    }
    if let Expr::Name(_, name) = &a.target {
        if let Some(existing) = fctx.lookup_innermost(name) {
            if a.op == AssignOp::Assign {
                check_expr(&a.value, Some(&existing), fctx, mctx)?;
            } else {
                check_compound_assign(a.op, &existing, &a.value, a.span, fctx, mctx)?;
            }
            return Ok(());
        }
        if a.op != AssignOp::Assign {
            return Err(type_error(
                "compound assignment requires an existing local".to_string(),
                a.span,
            ));
        }
        let ty = match &a.ty {
            Some(ann) => {
                let resolved = mctx.resolve_type(ann, &fctx.local_pools)?;
                check_expr(&a.value, Some(&resolved), fctx, mctx)?;
                resolved
            }
            None => check_expr(&a.value, None, fctx, mctx)?,
        };
        bind_local(fctx, name, ty, a.span)?;
        return Ok(());
    }
    // A non-name target (field, index) already exists; its type comes
    // from evaluating the place itself.
    let place_ty = check_expr(&a.target, None, fctx, mctx)?;
    if a.op == AssignOp::Assign {
        check_expr(&a.value, Some(&place_ty), fctx, mctx)?;
    } else {
        check_compound_assign(a.op, &place_ty, &a.value, a.span, fctx, mctx)?;
    }
    Ok(())
}

/// `a += b` desugars to `a = a.add(b)` (02-language.md §7.4): compute
/// `b`'s type checked against `a`'s current type (same-type operand
/// rule), run the same operator-resolution logic binary expressions use,
/// and require the result still fit back into `a`'s type (true
/// automatically for every builtin scalar op; for a user-type operator
/// method it holds exactly when the method's declared return type is the
/// operand type, the 05§8 shape).
fn check_compound_assign(
    op: AssignOp,
    target_ty: &Type,
    value: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let binop = match op {
        AssignOp::Add => BinOp::Add,
        AssignOp::Sub => BinOp::Sub,
        AssignOp::Mul => BinOp::Mul,
        AssignOp::Div => BinOp::Div,
        AssignOp::Rem => BinOp::Rem,
        AssignOp::BitAnd => BinOp::BitAnd,
        AssignOp::BitOr => BinOp::BitOr,
        AssignOp::BitXor => BinOp::BitXor,
        AssignOp::Shl => BinOp::Shl,
        AssignOp::Shr => BinOp::Shr,
        AssignOp::Assign => unreachable!("Assign never reaches check_compound_assign"),
    };
    check_expr(value, Some(target_ty), fctx, mctx)?;
    let result_ty = check_binop_types(binop, target_ty.clone(), span, mctx)?;
    if !types_eq(&result_ty, target_ty) {
        return Err(type_error(
            format!(
                "`{}` would change the type of the target from `{}` to `{}`",
                op.as_str(),
                types::render_type(target_ty),
                types::render_type(&result_ty)
            ),
            span,
        ));
    }
    Ok(())
}

// --- patterns (02-language.md §7.2) --------------------------------------

fn check_pattern(
    p: &Pattern,
    scrutinee: &Type,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    match p {
        Pattern::Wildcard(_) => Ok(()),
        Pattern::Literal(_span, expr) => {
            check_expr(expr, Some(scrutinee), fctx, mctx)?;
            Ok(())
        }
        Pattern::Binding(span, name) => bind_local(fctx, name, scrutinee.clone(), *span),
        Pattern::Take(_span, inner) => check_pattern(inner, scrutinee, fctx, mctx),
        Pattern::Variant {
            span,
            enum_name,
            variant,
            payload,
        } => {
            let payload_types =
                variant_payload_types_for(scrutinee, enum_name.as_deref(), variant, *span, mctx)?;
            if payload.len() != payload_types.len() {
                return Err(type_error(
                    format!(
                        "variant `{variant}` expects {} payload element(s), found {}",
                        payload_types.len(),
                        payload.len()
                    ),
                    *span,
                ));
            }
            for (sp, ty) in payload.iter().zip(payload_types.iter()) {
                check_pattern(sp, ty, fctx, mctx)?;
            }
            Ok(())
        }
        Pattern::Tuple(span, items) => {
            let Type::Tuple(elems) = scrutinee else {
                return Err(type_error(
                    format!(
                        "expected a tuple pattern for type `{}`",
                        types::render_type(scrutinee)
                    ),
                    *span,
                ));
            };
            if items.len() != elems.len() {
                return Err(type_error(
                    format!(
                        "tuple pattern expects {} element(s), found {}",
                        elems.len(),
                        items.len()
                    ),
                    *span,
                ));
            }
            for (sp, ty) in items.iter().zip(elems.iter()) {
                check_pattern(sp, ty, fctx, mctx)?;
            }
            Ok(())
        }
        Pattern::Array(span, items) => {
            let Type::Array(elem, len_expr) = scrutinee else {
                return Err(type_error(
                    format!(
                        "expected an array pattern for type `{}`",
                        types::render_type(scrutinee)
                    ),
                    *span,
                ));
            };
            if let Some(n) = literal_array_len(len_expr) {
                if n != items.len() as i128 {
                    return Err(type_error(
                        format!(
                            "array pattern expects {n} element(s), found {}",
                            items.len()
                        ),
                        *span,
                    ));
                }
            }
            for sp in items {
                check_pattern(sp, elem, fctx, mctx)?;
            }
            Ok(())
        }
        Pattern::Or(_span, alts) => {
            // Same-bindings-same-types across alternatives is item G's
            // job (exhaustiveness); each alternative is independently
            // well-formed against the scrutinee here.
            for alt in alts {
                check_pattern(alt, scrutinee, fctx, mctx)?;
            }
            Ok(())
        }
    }
}

fn literal_array_len(e: &Expr) -> Option<i128> {
    match e {
        Expr::Int(_, text) => parse_int_literal(text),
        _ => None, // needs comptime evaluation; skip the arity check rather than fail closed.
    }
}

/// Resolves a pattern's (or a leading-dot expression's) variant payload
/// types against the scrutinee/expected type: `Option`/`Result` are
/// builtin sums handled directly (their variants never route through
/// `mctx.enums`); a user enum's variants come from `mctx`. Anything else
/// cannot carry a variant pattern/construction.
fn variant_payload_types_for(
    scrutinee: &Type,
    enum_name: Option<&str>,
    variant: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<Vec<Type>, SemaError> {
    match scrutinee {
        Type::Option(inner) => {
            if let Some(n) = enum_name {
                if n != "Option" {
                    return Err(type_error(
                        format!("expected an `Option` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            match variant {
                "Some" => Ok(vec![(**inner).clone()]),
                "None" => Ok(vec![]),
                other => Err(type_error(
                    format!("`Option` has no variant `{other}`"),
                    span,
                )),
            }
        }
        Type::Result(ok, err) => {
            if let Some(n) = enum_name {
                if n != "Result" {
                    return Err(type_error(
                        format!("expected a `Result` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            match variant {
                "Ok" => Ok(vec![(**ok).clone()]),
                "Err" => Ok(vec![(**err).clone()]),
                other => Err(type_error(
                    format!("`Result` has no variant `{other}`"),
                    span,
                )),
            }
        }
        Type::Named(name, targs) => {
            if !targs.is_empty() {
                return Err(unimplemented_at("generic instantiation is", span));
            }
            if let Some(n) = enum_name {
                if n != name {
                    return Err(type_error(
                        format!("expected a `{name}` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            let Some(e) = mctx.enums.get(name) else {
                return Err(type_error(format!("`{name}` is not an enum"), span));
            };
            let Some(dv) = e.variants.iter().find(|v| v.name == variant) else {
                return Err(type_error(
                    format!("enum `{name}` has no variant `{variant}`"),
                    span,
                ));
            };
            Ok(decl_variant_payload_types(dv))
        }
        other => Err(type_error(
            format!(
                "cannot match a variant pattern against type `{}`",
                types::render_type(other)
            ),
            span,
        )),
    }
}

fn decl_variant_payload_types(dv: &types::DeclVariant) -> Vec<Type> {
    match &dv.payload {
        DeclVariantPayload::None => vec![],
        DeclVariantPayload::Tuple(types_) => types_.clone(),
        DeclVariantPayload::Named(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
    }
}

// --- expressions: the central check/synth pair ---------------------------

/// Checks `expr` against `expected` (decision 4): synthesizes its type
/// (`synth_expr`, which uses `expected` internally wherever the grammar
/// needs it — literal defaulting, closures, `Some`/`Ok`/`Err`/leading-dot
/// construction, array/tuple literals), then gates the result against
/// `expected` when one was supplied. Always returns the actual type, so
/// callers that need it (call-argument checking, `for`'s range endpoints,
/// ...) get it back either way.
fn check_expr(
    expr: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    let actual = synth_expr(expr, expected, fctx, mctx)?;
    if let Some(exp) = expected {
        if !types_eq(&actual, exp) {
            return Err(type_error(
                format!(
                    "expected `{}`, found `{}`",
                    types::render_type(exp),
                    types::render_type(&actual)
                ),
                expr.span(),
            ));
        }
    }
    Ok(actual)
}

fn synth_expr(
    expr: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    match expr {
        Expr::Int(span, text) => synth_int_literal(*span, text, expected),
        Expr::Float(span, _text) => synth_float_literal(*span, expected),
        Expr::Str(_span, _text) => Ok(Type::Static(Box::new(Type::Str))),
        Expr::BStr(span, text) => {
            let len = bstr_byte_len(text);
            Ok(Type::Static(Box::new(Type::Bytes(Some(Box::new(
                Expr::Int(*span, len.to_string()),
            ))))))
        }
        Expr::Char(_span, _) => Ok(Type::Char),
        Expr::FStr(f) => Err(unimplemented_at("f-strings are", f.span)),
        Expr::Bool(_span, _) => Ok(Type::Bool),
        Expr::Unit(_span) => Ok(Type::Unit),
        Expr::Name(span, name) => synth_name(*span, name, expected, fctx, mctx),
        Expr::Field(base, span, name) => check_field_expr(base, *span, name, fctx, mctx),
        Expr::Index(base, span, args) => synth_index(base, *span, args, fctx, mctx),
        Expr::Call(callee, span, args) => check_call(callee, *span, args, expected, fctx, mctx),
        Expr::Unary(span, UnaryOp::Neg, inner) => {
            check_unary_neg(inner, expected, *span, fctx, mctx)
        }
        Expr::Unary(span, UnaryOp::BitNot, inner) => {
            let ty = check_expr(inner, expected, fctx, mctx)?;
            if !is_integer_scalar(&ty) {
                return Err(type_error(
                    format!(
                        "`~` requires an integer type, found `{}`",
                        types::render_type(&ty)
                    ),
                    *span,
                ));
            }
            Ok(ty)
        }
        Expr::Unary(span, UnaryOp::Await, _inner) => Err(unimplemented_at("await is", *span)),
        Expr::Unary(_span, UnaryOp::Take, inner) => check_expr(inner, expected, fctx, mctx),
        Expr::Try(span, inner) => check_try(*span, inner, fctx, mctx),
        Expr::Binary(span, op, l, r) => check_binary(*op, l, r, *span, fctx, mctx),
        Expr::Range(span, _from, _to, _incl) => Err(type_error(
            "a range is only a value directly inside `for`".to_string(),
            *span,
        )),
        Expr::Is(_span, scrutinee, pattern) => {
            let sty = check_expr(scrutinee, None, fctx, mctx)?;
            check_pattern(pattern, &sty, fctx, mctx)?;
            Ok(Type::Bool)
        }
        Expr::Not(_span, inner) => {
            check_expr(inner, Some(&Type::Bool), fctx, mctx)?;
            Ok(Type::Bool)
        }
        Expr::And(_span, l, r) | Expr::Or(_span, l, r) => {
            check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(Type::Bool)
        }
        Expr::DotVariant(span, name, args) => {
            let Some(exp) = expected else {
                return Err(type_error(
                    format!("cannot infer an enum type for `.{name}`"),
                    *span,
                ));
            };
            let exp = exp.clone();
            let payload_types = variant_payload_types_for(&exp, None, name, *span, mctx)?;
            check_variant_args(&payload_types, args, *span, fctx, mctx)?;
            Ok(exp)
        }
        Expr::Closure(c) => check_closure(c, expected, fctx, mctx),
        Expr::Send(span, _inner) => Err(unimplemented_at("send is", *span)),
        Expr::Tuple(span, items) => synth_tuple(*span, items, expected, fctx, mctx),
        Expr::List(span, items) => synth_list(*span, items, expected, fctx, mctx),
    }
}

fn synth_int_literal(span: Span, text: &str, expected: Option<&Type>) -> Result<Type, SemaError> {
    let value = parse_int_literal(text)
        .ok_or_else(|| type_error("invalid integer literal".to_string(), span))?;
    match expected {
        Some(t) if is_integer_scalar(t) => {
            check_int_range(value, t, span)?;
            Ok(t.clone())
        }
        Some(t) => Err(type_error(
            format!(
                "expected `{}`, found an integer literal",
                types::render_type(t)
            ),
            span,
        )),
        None => {
            if value <= i64::MAX as i128 {
                Ok(Type::I64)
            } else if value <= u64::MAX as i128 {
                Ok(Type::U64)
            } else {
                Err(type_error("integer literal out of range".to_string(), span))
            }
        }
    }
}

fn synth_float_literal(span: Span, expected: Option<&Type>) -> Result<Type, SemaError> {
    match expected {
        Some(t) if is_float_scalar(t) => Ok(t.clone()),
        Some(t) => Err(type_error(
            format!(
                "expected `{}`, found a float literal",
                types::render_type(t)
            ),
            span,
        )),
        None => Ok(Type::F64),
    }
}

fn synth_name(
    span: Span,
    name: &str,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Some(ty) = fctx.lookup_local(name) {
        return Ok(ty);
    }
    if let Some(ty) = mctx.consts.get(name) {
        return Ok(ty.clone());
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            return Err(unimplemented_at("generic instantiation is", span));
        }
        return Ok(fn_value_type(&f.decl));
    }
    if mctx.structs.contains_key(name) || mctx.enums.contains_key(name) {
        return Err(type_error(format!("`{name}` is a type, not a value"), span));
    }
    match name {
        "None" => match expected {
            Some(t @ Type::Option(_)) => Ok(t.clone()),
            _ => Err(type_error(
                "cannot infer the type of `None` without context".to_string(),
                span,
            )),
        },
        "Some" | "Ok" | "Err" | "panic" => Err(type_error(
            format!("`{name}` cannot be used without being called"),
            span,
        )),
        _ => Err(type_error(
            format!("cannot determine the type of `{name}`"),
            span,
        )),
    }
}

fn fn_value_type(d: &types::DeclFn) -> Type {
    let params = d.params.iter().map(|p| (p.mode, p.ty.clone())).collect();
    Type::Fn(params, Box::new(d.ret.clone()))
}

fn unwrap_own(ty: Type) -> Type {
    match ty {
        Type::Own(_, inner) => *inner,
        other => other,
    }
}

fn check_field_expr(
    base: &Expr,
    span: Span,
    name: &str,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                if let Some((_, d)) = s.assoc_fn(name) {
                    return Ok(fn_value_type(d));
                }
                if s.has_member_named(name) {
                    return Err(type_error(
                        format!("cannot reference method `{name}` without calling it"),
                        span,
                    ));
                }
                return Err(type_error(
                    format!("type `{bname}` has no associated function `{name}`"),
                    span,
                ));
            }
            if let Some(e) = mctx.enums.get(bname.as_str()) {
                if !e.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                if let Some(dv) = e.variants.iter().find(|v| v.name == name) {
                    if matches!(dv.payload, DeclVariantPayload::None) {
                        return Ok(Type::Named(e.name.clone(), vec![]));
                    }
                    return Err(type_error(
                        format!("variant `{name}` requires a payload"),
                        span,
                    ));
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant `{name}`"),
                    span,
                ));
            }
        }
    }
    let base_ty = unwrap_own(check_expr(base, None, fctx, mctx)?);
    match &base_ty {
        Type::Named(sname, targs) => {
            if !targs.is_empty() {
                return Err(unimplemented_at("generic instantiation is", span));
            }
            let Some(s) = mctx.structs.get(sname.as_str()) else {
                return Err(type_error(
                    format!("type `{sname}` has no field `{name}`"),
                    span,
                ));
            };
            if let Some(ty) = s.field_ty(name) {
                return Ok(ty);
            }
            if s.has_member_named(name) {
                return Err(type_error(
                    format!("cannot reference method `{name}` without calling it"),
                    span,
                ));
            }
            Err(type_error(
                format!("type `{sname}` has no field `{name}`"),
                span,
            ))
        }
        other => Err(type_error(
            format!("type `{}` has no field `{name}`", types::render_type(other)),
            span,
        )),
    }
}

fn synth_index(
    base: &Expr,
    span: Span,
    args: &[Expr],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Expr::Name(_, n) = base {
        if mctx.structs.contains_key(n) || mctx.enums.contains_key(n) || mctx.fns.contains_key(n) {
            return Err(unimplemented_at("generic instantiation is", span));
        }
    }
    let base_ty = unwrap_own(check_expr(base, None, fctx, mctx)?);
    if args.len() != 1 {
        return Err(type_error(
            format!("indexing takes exactly one argument, found {}", args.len()),
            span,
        ));
    }
    match &base_ty {
        Type::Array(elem, _) => {
            check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?;
            Ok((**elem).clone())
        }
        Type::Bytes(_) => {
            check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?;
            Ok(Type::U8)
        }
        other => Err(type_error(
            format!("type `{}` cannot be indexed", types::render_type(other)),
            span,
        )),
    }
}

fn synth_tuple(
    span: Span,
    items: &[Expr],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Some(Type::Tuple(exp_elems)) = expected {
        if exp_elems.len() != items.len() {
            return Err(type_error(
                format!(
                    "tuple expects {} element(s), found {}",
                    exp_elems.len(),
                    items.len()
                ),
                span,
            ));
        }
        let exp_elems = exp_elems.clone();
        for (item, ety) in items.iter().zip(exp_elems.iter()) {
            check_expr(item, Some(ety), fctx, mctx)?;
        }
        return Ok(Type::Tuple(exp_elems));
    }
    let mut elems = Vec::with_capacity(items.len());
    for item in items {
        elems.push(check_expr(item, None, fctx, mctx)?);
    }
    Ok(Type::Tuple(elems))
}

fn synth_list(
    span: Span,
    items: &[Expr],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Some(Type::Array(elem, len_expr)) = expected {
        let elem = (**elem).clone();
        let len_expr = len_expr.clone();
        if let Some(n) = literal_array_len(&len_expr) {
            if n != items.len() as i128 {
                return Err(type_error(
                    format!("array expects {n} element(s), found {}", items.len()),
                    span,
                ));
            }
        }
        for item in items {
            check_expr(item, Some(&elem), fctx, mctx)?;
        }
        return Ok(Type::Array(Box::new(elem), len_expr));
    }
    if items.is_empty() {
        return Err(type_error(
            "cannot infer the element type of an empty array literal".to_string(),
            span,
        ));
    }
    let first = check_expr(&items[0], None, fctx, mctx)?;
    for item in &items[1..] {
        check_expr(item, Some(&first), fctx, mctx)?;
    }
    let len = Expr::Int(span, items.len().to_string());
    Ok(Type::Array(Box::new(first), Box::new(len)))
}

// --- unary `-`, binary operators (02-language.md §7.4, §8.2; 05-library.md §8) --

fn is_integer_scalar(t: &Type) -> bool {
    matches!(
        t,
        Type::U8
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

fn is_signed_scalar(t: &Type) -> bool {
    matches!(
        t,
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Isize
    )
}

fn is_float_scalar(t: &Type) -> bool {
    matches!(t, Type::F32 | Type::F64)
}

fn is_numeric_scalar(t: &Type) -> bool {
    is_integer_scalar(t) || is_float_scalar(t)
}

/// Structural type equality, used everywhere in place of derived
/// `PartialEq` on `Type`: `Type::Array`/`Type::Bytes` embed their length
/// as an unevaluated `ast::Expr` (types.rs, item H evaluates the literal
/// subset), and `Expr`'s derived `PartialEq` also compares spans — so
/// the *same* `[T; 3]` written at two different source locations would
/// otherwise never compare equal. `same_len_expr` below compares length
/// expressions by value/name instead, ignoring span.
fn types_eq(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Bool, Type::Bool)
        | (Type::U8, Type::U8)
        | (Type::U16, Type::U16)
        | (Type::U32, Type::U32)
        | (Type::U64, Type::U64)
        | (Type::Usize, Type::Usize)
        | (Type::I8, Type::I8)
        | (Type::I16, Type::I16)
        | (Type::I32, Type::I32)
        | (Type::I64, Type::I64)
        | (Type::Isize, Type::Isize)
        | (Type::F32, Type::F32)
        | (Type::F64, Type::F64)
        | (Type::Char, Type::Char)
        | (Type::Unit, Type::Unit)
        | (Type::Never, Type::Never)
        | (Type::Str, Type::Str) => true,
        (Type::Array(e1, l1), Type::Array(e2, l2)) => types_eq(e1, e2) && same_len_expr(l1, l2),
        (Type::Tuple(a1), Type::Tuple(a2)) => {
            a1.len() == a2.len() && a1.iter().zip(a2.iter()).all(|(x, y)| types_eq(x, y))
        }
        (Type::Option(x), Type::Option(y)) => types_eq(x, y),
        (Type::Result(a1, b1), Type::Result(a2, b2)) => types_eq(a1, a2) && types_eq(b1, b2),
        (Type::Own(p1, t1), Type::Own(p2, t2)) => p1 == p2 && types_eq(t1, t2),
        (Type::Static(x), Type::Static(y)) => types_eq(x, y),
        (Type::Bytes(None), Type::Bytes(None)) => true,
        (Type::Bytes(Some(l1)), Type::Bytes(Some(l2))) => same_len_expr(l1, l2),
        (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
            p1.len() == p2.len()
                && p1
                    .iter()
                    .zip(p2.iter())
                    .all(|((m1, t1), (m2, t2))| m1 == m2 && types_eq(t1, t2))
                && types_eq(r1, r2)
        }
        (Type::Generic(n1), Type::Generic(n2)) => n1 == n2,
        (Type::Named(n1, a1), Type::Named(n2, a2)) => {
            n1 == n2
                && a1.len() == a2.len()
                && a1.iter().zip(a2.iter()).all(|(x, y)| type_args_eq(x, y))
        }
        _ => false,
    }
}

fn type_args_eq(a: &types::TypeArg, b: &types::TypeArg) -> bool {
    match (a, b) {
        (types::TypeArg::Type(x), types::TypeArg::Type(y)) => types_eq(x, y),
        (types::TypeArg::Const(x), types::TypeArg::Const(y)) => same_len_expr(x, y),
        (types::TypeArg::Bound(x), types::TypeArg::Bound(y)) => same_len_expr(x, y),
        _ => false,
    }
}

/// Only the two shapes an M2 length/const argument actually takes — a
/// literal integer or a bare `const`/generic-param name — compare by
/// value; anything else is conservatively unequal (comparing two
/// arbitrary expressions honestly needs comptime evaluation, item M3).
fn same_len_expr(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Int(_, t1), Expr::Int(_, t2)) => parse_int_literal(t1) == parse_int_literal(t2),
        (Expr::Name(_, n1), Expr::Name(_, n2)) => n1 == n2,
        _ => false,
    }
}

fn int_bounds(ty: &Type) -> Option<(i128, i128)> {
    match ty {
        Type::U8 => Some((0, u8::MAX as i128)),
        Type::U16 => Some((0, u16::MAX as i128)),
        Type::U32 => Some((0, u32::MAX as i128)),
        Type::U64 | Type::Usize => Some((0, u64::MAX as i128)),
        Type::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        Type::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        Type::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        Type::I64 | Type::Isize => Some((i64::MIN as i128, i64::MAX as i128)),
        _ => None,
    }
}

fn check_int_range(value: i128, ty: &Type, span: Span) -> Result<(), SemaError> {
    let (min, max) = int_bounds(ty).expect("check_int_range called with a non-integer type");
    if value < min || value > max {
        return Err(type_error(
            format!(
                "integer literal out of range for `{}`",
                types::render_type(ty)
            ),
            span,
        ));
    }
    Ok(())
}

fn parse_int_literal(text: &str) -> Option<i128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits): (u32, &str) = if let Some(d) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, d)
    } else if let Some(d) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, d)
    } else if let Some(d) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, d)
    } else {
        (10, cleaned.as_str())
    };
    i128::from_str_radix(digits, radix).ok()
}

/// Decoded byte length of a byte-string literal's raw (still-escaped)
/// source text (lexer.rs: "contents kept raw"): each escape (already
/// validated at lex time — `\xNN`, or one of `\\ \" \' \n \r \t \0`)
/// contributes exactly one byte; anything else contributes its own
/// UTF-8 length.
fn bstr_byte_len(text: &str) -> u64 {
    let mut len = 0u64;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('x') => {
                    chars.next();
                    chars.next();
                    len += 1;
                }
                Some(_) => len += 1,
                None => {}
            }
        } else {
            len += c.len_utf8() as u64;
        }
    }
    len
}

fn check_unary_neg(
    inner: &Expr,
    expected: Option<&Type>,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    match inner {
        Expr::Int(ispan, text) => {
            let raw = parse_int_literal(text)
                .ok_or_else(|| type_error("invalid integer literal".to_string(), *ispan))?;
            let value = -raw;
            match expected {
                Some(t) if is_integer_scalar(t) => {
                    check_int_range(value, t, *ispan)?;
                    Ok(t.clone())
                }
                Some(t) => Err(type_error(
                    format!(
                        "expected `{}`, found an integer literal",
                        types::render_type(t)
                    ),
                    *ispan,
                )),
                None => {
                    check_int_range(value, &Type::I64, *ispan)?;
                    Ok(Type::I64)
                }
            }
        }
        Expr::Float(_, _) => synth_float_literal(inner.span(), expected),
        _ => {
            let ty = check_expr(inner, expected, fctx, mctx)?;
            if (is_integer_scalar(&ty) && is_signed_scalar(&ty)) || is_float_scalar(&ty) {
                Ok(ty)
            } else {
                Err(type_error(
                    format!(
                        "unary `-` requires a signed integer or float type, found `{}`",
                        types::render_type(&ty)
                    ),
                    span,
                ))
            }
        }
    }
}

fn check_binary(
    op: BinOp,
    l: &Expr,
    r: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    let ty = check_same_type_operands(l, r, fctx, mctx)?;
    check_binop_types(op, ty, span, mctx)
}

/// Checks two operands that must share one type (a binary operator's
/// sides, a range's endpoints), with no unification (decision 4): one
/// side is synthesized on its own, then the other is checked against it.
/// A bare, unannotated integer/float literal defers to a concrete
/// sibling when there is one — `0 .. n` (or `n + 1`) types the literal
/// against `n`'s type rather than defaulting it first and rejecting `n`
/// — so ordinary code with the literal on either side works the same
/// way; only two bare literals together fall back to plain left-to-right
/// (both then default identically, so it never matters which is first).
fn check_same_type_operands(
    a: &Expr,
    b: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    let (first, second) = if is_bare_numeric_literal(a) && !is_bare_numeric_literal(b) {
        (b, a)
    } else {
        (a, b)
    };
    let ty = check_expr(first, None, fctx, mctx)?;
    check_expr(second, Some(&ty), fctx, mctx)?;
    Ok(ty)
}

fn is_bare_numeric_literal(e: &Expr) -> bool {
    matches!(e, Expr::Int(..) | Expr::Float(..))
}

/// Both operands already share `ty` by the time this runs
/// (`check_binary` calls `check_same_type_operands`;
/// `check_compound_assign` checks the value against the target's type).
/// Builtin scalar ops never desugar (02-language.md §7.4); a user
/// (`Named`) type's `+ - * / %` and `<` resolve to the matching 05§8
/// method; everything else in the table (wrapping, shifts, bitwise) is
/// core-scalar-only.
fn check_binop_types(op: BinOp, ty: Type, span: Span, mctx: &ModuleCtx) -> Result<Type, SemaError> {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Rem => {
            if is_numeric_scalar(&ty) {
                return Ok(ty);
            }
            if let Type::Named(name, targs) = &ty {
                if !targs.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                let method = match op {
                    Add => "add",
                    Sub => "subtract",
                    Mul => "multiply",
                    Div => "divide",
                    Rem => "remainder",
                    _ => unreachable!(),
                };
                return resolve_operator_method(name, method, &ty, mctx, span);
            }
            Err(type_error(
                format!(
                    "operator `{}` is not supported for type `{}`",
                    op.as_str(),
                    types::render_type(&ty)
                ),
                span,
            ))
        }
        AddW | SubW | MulW => {
            if is_integer_scalar(&ty) {
                Ok(ty)
            } else {
                Err(type_error(
                    format!(
                        "wrapping arithmetic requires an integer type, found `{}`",
                        types::render_type(&ty)
                    ),
                    span,
                ))
            }
        }
        Shl | Shr | BitAnd | BitOr | BitXor => {
            if is_integer_scalar(&ty) {
                Ok(ty)
            } else {
                Err(type_error(
                    format!(
                        "`{}` requires an integer type, found `{}`",
                        op.as_str(),
                        types::render_type(&ty)
                    ),
                    span,
                ))
            }
        }
        Lt | Le | Gt | Ge => {
            if is_numeric_scalar(&ty) || matches!(ty, Type::Char) {
                return Ok(Type::Bool);
            }
            if let Type::Named(name, targs) = &ty {
                if !targs.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                if op == Lt {
                    let ret = resolve_operator_method(name, "less_than", &ty, mctx, span)?;
                    if ret != Type::Bool {
                        return Err(type_error(
                            format!("`{name}.less_than` must return `bool`"),
                            span,
                        ));
                    }
                    return Ok(Type::Bool);
                }
                return Err(unimplemented_at(
                    "derived comparisons (`<=`, `>`, `>=`) on a user type are",
                    span,
                ));
            }
            Err(type_error(
                format!(
                    "comparison is not supported for type `{}`",
                    types::render_type(&ty)
                ),
                span,
            ))
        }
        Eq | Ne => {
            if is_resource_type(&ty, mctx) {
                return Err(type_error(
                    format!(
                        "cannot compare resource type `{}` with `==`",
                        types::render_type(&ty)
                    ),
                    span,
                ));
            }
            Ok(Type::Bool)
        }
    }
}

/// Mirrors `types::classify_type` (already computed, memoized, per
/// struct/enum in `mctx`) to answer the one question the operator pass
/// needs: is this composite type's structural `==` forbidden because it
/// (transitively) contains a resource?
fn is_resource_type(ty: &Type, mctx: &ModuleCtx) -> bool {
    match ty {
        Type::Own(..) => true,
        Type::Static(_) => false,
        Type::Named(name, _) => mctx
            .structs
            .get(name)
            .map(|s| s.decl.classification == Classification::Resource)
            .or_else(|| {
                mctx.enums
                    .get(name)
                    .map(|e| e.classification == Classification::Resource)
            })
            .unwrap_or(false),
        Type::Array(elem, _) => is_resource_type(elem, mctx),
        Type::Tuple(elems) => elems.iter().any(|e| is_resource_type(e, mctx)),
        Type::Option(inner) => is_resource_type(inner, mctx),
        Type::Result(ok, err) => is_resource_type(ok, mctx) || is_resource_type(err, mctx),
        _ => false,
    }
}

/// Resolves `<type-name>.<method>` as an operator-desugar target
/// (05-library.md §8 shape: `fn <method>(read self, right: <Self>) ->
/// R`), returning the method's declared result type `R`.
fn resolve_operator_method(
    name: &str,
    method: &str,
    self_ty: &Type,
    mctx: &ModuleCtx,
    span: Span,
) -> Result<Type, SemaError> {
    let Some(s) = mctx.structs.get(name) else {
        return Err(type_error(
            format!("type `{name}` has no operator method `{method}`"),
            span,
        ));
    };
    let Some((_, d)) = s.method(method) else {
        return Err(type_error(
            format!("type `{name}` has no operator method `{method}`"),
            span,
        ));
    };
    let receiver_read = d
        .receiver
        .as_ref()
        .map(|r| r.mode == AccessMode::Read)
        .unwrap_or(false);
    let shape_ok = receiver_read
        && d.generics.is_empty()
        && d.params.len() == 1
        && types_eq(&d.params[0].ty, self_ty);
    if !shape_ok {
        return Err(type_error(
            format!(
                "`{name}.{method}` does not match the operator method shape `{method}(read self, right: {name}) -> ...`"
            ),
            span,
        ));
    }
    Ok(d.ret.clone())
}

// --- `?` (02-language.md §7.4, §8.2; 05-library.md §1) --------------------

fn check_try(
    span: Span,
    inner: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    let inner_ty = check_expr(inner, None, fctx, mctx)?;
    match inner_ty {
        Type::Result(t_ok, t_err) => match fctx.ret_ty.clone() {
            Type::Result(_, ret_err) => {
                if types_eq(&t_err, &ret_err) {
                    Ok(*t_ok)
                } else if let Some(conv_ret) = try_from_conversion(&t_err, &ret_err, mctx) {
                    if types_eq(&conv_ret, &ret_err) {
                        Ok(*t_ok)
                    } else {
                        Err(type_error(
                            format!(
                                "`?` conversion `from` must return `{}`, found `{}`",
                                types::render_type(&ret_err),
                                types::render_type(&conv_ret)
                            ),
                            span,
                        ))
                    }
                } else {
                    Err(type_error(
                        format!(
                            "`?` cannot convert error type `{}` to `{}`",
                            types::render_type(&t_err),
                            types::render_type(&ret_err)
                        ),
                        span,
                    ))
                }
            }
            _ => Err(type_error(
                "`?` on a `Result` requires an enclosing function returning `Result`".to_string(),
                span,
            )),
        },
        Type::Option(t_inner) => match &fctx.ret_ty {
            Type::Option(_) => Ok(*t_inner),
            _ => Err(type_error(
                "`?` on an `Option` requires an enclosing function returning `Option`".to_string(),
                span,
            )),
        },
        other => Err(type_error(
            format!(
                "`?` requires a `Result` or `Option`, found `{}`",
                types::render_type(&other)
            ),
            span,
        )),
    }
}

/// The one hop `?` may take (02-language.md §7.4: "no chains, no
/// implicit widening"): `target_ty` either matches `err_ty` directly
/// (checked by the caller before this runs) or names a struct/enum
/// declaring the conversion — a user-written associated `from(take
/// source: E) -> Self`, or the equivalent `deriving(From)` generates
/// (05-library.md §8) from its single field/payload.
fn try_from_conversion(err_ty: &Type, target_ty: &Type, mctx: &ModuleCtx) -> Option<Type> {
    let Type::Named(name, targs) = target_ty else {
        return None;
    };
    if !targs.is_empty() {
        return None;
    }
    if let Some(s) = mctx.structs.get(name) {
        if s.decl.deriving.iter().any(|d| d == "From") {
            let field_ty = s.decl.members.iter().find_map(|m| match m {
                DeclMember::Field(f) => Some(f.ty.clone()),
                _ => None,
            });
            if let Some(ft) = field_ty {
                if types_eq(&ft, err_ty) {
                    return Some(target_ty.clone());
                }
            }
        }
        if let Some((_, d)) = s.assoc_fn("from") {
            let shape_ok = d.generics.is_empty()
                && d.params.len() == 1
                && d.params[0].mode == AccessMode::Take
                && types_eq(&d.params[0].ty, err_ty);
            if shape_ok {
                return Some(d.ret.clone());
            }
        }
    }
    if let Some(e) = mctx.enums.get(name) {
        if e.deriving.iter().any(|d| d == "From") {
            if let Some(dv) = e.variants.first() {
                if let Some(pt) = decl_variant_payload_types(dv).into_iter().next() {
                    if types_eq(&pt, err_ty) {
                        return Some(target_ty.clone());
                    }
                }
            }
        }
    }
    None
}

// --- closures (02-language.md §8.3) --------------------------------------

fn check_closure(
    c: &ClosureExpr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    let Some(Type::Fn(exp_params, exp_ret)) = expected.cloned() else {
        return Err(type_error(
            "a closure needs a known function type from its call-site context".to_string(),
            c.span,
        ));
    };
    if c.params.len() != exp_params.len() {
        return Err(arity_error(exp_params.len(), c.params.len(), c.span));
    }
    fctx.push_scope();
    let result = check_closure_body(c, &exp_params, &exp_ret, fctx, mctx);
    fctx.pop_scope();
    result?;
    Ok(Type::Fn(exp_params, exp_ret))
}

fn check_closure_body(
    c: &ClosureExpr,
    exp_params: &[(AccessMode, Type)],
    exp_ret: &Type,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    for (cp, (_mode, ety)) in c.params.iter().zip(exp_params.iter()) {
        let pty = match &cp.ty {
            Some(t) => {
                let resolved = mctx.resolve_type(t, &fctx.local_pools)?;
                if !types_eq(&resolved, ety) {
                    return Err(type_error(
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
    match &c.body {
        ClosureBody::Expr(e) => {
            check_expr(e, Some(exp_ret), fctx, mctx)?;
        }
        ClosureBody::Suite(stmts) => {
            let saved_ret = std::mem::replace(&mut fctx.ret_ty, exp_ret.clone());
            let r = check_stmts(stmts, fctx, mctx);
            fctx.ret_ty = saved_ret;
            r?;
        }
    }
    Ok(())
}

// --- calls: fn/method/associated-fn/init/struct-literal/enum-variant ----

fn call_fn_value(
    ty: Type,
    args: &[Arg],
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    match ty {
        Type::Fn(params, ret) => {
            check_positional_args(&params, args, span, fctx, mctx)?;
            Ok(*ret)
        }
        other => Err(type_error(
            format!("type `{}` is not callable", types::render_type(&other)),
            span,
        )),
    }
}

fn check_call(
    callee: &Expr,
    span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    match callee {
        Expr::Index(inner, ispan, targs) => {
            check_call_index(inner, *ispan, targs, args, span, fctx, mctx)
        }
        Expr::Name(nspan, name) => {
            check_call_by_name(name, *nspan, span, args, expected, fctx, mctx)
        }
        Expr::Field(base, fspan, name) => {
            check_call_by_field(base, *fspan, name, span, args, fctx, mctx)
        }
        other => {
            let ty = check_expr(other, None, fctx, mctx)?;
            call_fn_value(ty, args, span, fctx, mctx)
        }
    }
}

/// Callee shaped `expr[targs](args)` — either a scalar conversion
/// (`x.to[T]()`, `x.checked_to[T]()`, `x.truncate_to[T]()`) or a generic
/// instantiation (`Ring[Sector, 4](...)`, `hash_pair[Sector](...)`); the
/// latter always fails closed (item H).
fn check_call_index(
    inner: &Expr,
    ispan: Span,
    targs: &[Expr],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Expr::Field(base, fspan, mname) = inner {
        if mname == "to" || mname == "checked_to" || mname == "truncate_to" {
            if targs.len() != 1 {
                return Err(type_error(
                    "a conversion needs exactly one type argument".to_string(),
                    ispan,
                ));
            }
            let base_ty = check_expr(base, None, fctx, mctx)?;
            if !is_scalar(&base_ty) {
                return Err(type_error(
                    format!(
                        "`.{mname}` is only defined for scalar types, found `{}`",
                        types::render_type(&base_ty)
                    ),
                    *fspan,
                ));
            }
            if !args.is_empty() {
                return Err(type_error(
                    format!("`.{mname}()` takes no arguments"),
                    call_span,
                ));
            }
            if mname == "checked_to" {
                return Err(unimplemented_at("checked_to conversion is", call_span));
            }
            if mname == "truncate_to" {
                return Err(unimplemented_at("truncate_to conversion is", call_span));
            }
            let target = scalar_type_by_name_expr(&targs[0]).ok_or_else(|| {
                type_error("`.to` target must be a scalar type".to_string(), ispan)
            })?;
            return Ok(target);
        }
    }
    Err(unimplemented_at("generic instantiation is", call_span))
}

fn scalar_type_by_name_expr(e: &Expr) -> Option<Type> {
    match e {
        Expr::Name(_, name) => scalar_type_by_name(name),
        _ => None,
    }
}

fn scalar_type_by_name(name: &str) -> Option<Type> {
    Some(match name {
        "bool" => Type::Bool,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "usize" => Type::Usize,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "isize" => Type::Isize,
        "f32" => Type::F32,
        "f64" => Type::F64,
        "char" => Type::Char,
        _ => return None,
    })
}

fn is_scalar(t: &Type) -> bool {
    is_numeric_scalar(t) || matches!(t, Type::Bool | Type::Char)
}

fn check_call_by_name(
    name: &str,
    nspan: Span,
    call_span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Some(ty) = fctx.lookup_local(name) {
        return call_fn_value(ty, args, call_span, fctx, mctx);
    }
    if let Some(c) = mctx.consts.get(name) {
        let ty = c.clone();
        return call_fn_value(ty, args, call_span, fctx, mctx);
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            return Err(unimplemented_at("generic instantiation is", call_span));
        }
        check_call_args(&f.ast.params, &f.decl.params, args, call_span, fctx, mctx)?;
        return Ok(f.decl.ret.clone());
    }
    if let Some(s) = mctx.structs.get(name) {
        if !s.decl.generics.is_empty() {
            return Err(unimplemented_at("generic instantiation is", call_span));
        }
        return check_struct_construction(s, args, call_span, fctx, mctx);
    }
    match name {
        "Some" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let inner_expected = match expected {
                Some(Type::Option(inner)) => Some((**inner).clone()),
                _ => None,
            };
            let ity = check_expr(&args[0].value, inner_expected.as_ref(), fctx, mctx)?;
            Ok(Type::Option(Box::new(ity)))
        }
        "Ok" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let (t_expected, e_ty) = match expected {
                Some(Type::Result(t, e)) => (Some((**t).clone()), Some((**e).clone())),
                _ => (None, None),
            };
            let t_ty = check_expr(&args[0].value, t_expected.as_ref(), fctx, mctx)?;
            let e_ty = e_ty.ok_or_else(|| {
                type_error(
                    "cannot infer the error type of `Ok(...)` without context".to_string(),
                    call_span,
                )
            })?;
            Ok(Type::Result(Box::new(t_ty), Box::new(e_ty)))
        }
        "Err" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let (t_ty_opt, e_expected) = match expected {
                Some(Type::Result(t, e)) => (Some((**t).clone()), Some((**e).clone())),
                _ => (None, None),
            };
            let e_ty = check_expr(&args[0].value, e_expected.as_ref(), fctx, mctx)?;
            let t_ty = t_ty_opt.ok_or_else(|| {
                type_error(
                    "cannot infer the ok type of `Err(...)` without context".to_string(),
                    call_span,
                )
            })?;
            Ok(Type::Result(Box::new(t_ty), Box::new(e_ty)))
        }
        "panic" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            check_expr(
                &args[0].value,
                Some(&Type::Static(Box::new(Type::Str))),
                fctx,
                mctx,
            )?;
            Ok(Type::Never)
        }
        _ => {
            let _ = nspan;
            Err(type_error(format!("`{name}` is not callable"), call_span))
        }
    }
}

fn check_call_by_field(
    base: &Expr,
    fspan: Span,
    name: &str,
    call_span: Span,
    args: &[Arg],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", call_span));
                }
                if let Some((af, d)) = s.assoc_fn(name) {
                    if !d.generics.is_empty() {
                        return Err(unimplemented_at("generic instantiation is", call_span));
                    }
                    check_call_args(&af.params, &d.params, args, call_span, fctx, mctx)?;
                    return Ok(d.ret.clone());
                }
                return Err(type_error(
                    format!("type `{bname}` has no associated function `{name}`"),
                    fspan,
                ));
            }
            if let Some(e) = mctx.enums.get(bname.as_str()) {
                if !e.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", call_span));
                }
                if let Some(dv) = e.variants.iter().find(|v| v.name == name) {
                    let payload_types = decl_variant_payload_types(dv);
                    check_variant_args(&payload_types, args, call_span, fctx, mctx)?;
                    return Ok(Type::Named(e.name.clone(), vec![]));
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant `{name}`"),
                    fspan,
                ));
            }
        }
    }
    let base_ty = unwrap_own(check_expr(base, None, fctx, mctx)?);
    match &base_ty {
        Type::Named(sname, targs) => {
            if !targs.is_empty() {
                return Err(unimplemented_at("generic instantiation is", call_span));
            }
            let Some(s) = mctx.structs.get(sname.as_str()) else {
                return Err(type_error(
                    format!("type `{sname}` has no method `{name}`"),
                    fspan,
                ));
            };
            let Some((mf, d)) = s.method(name) else {
                return Err(type_error(
                    format!("type `{sname}` has no method `{name}`"),
                    fspan,
                ));
            };
            if !d.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", call_span));
            }
            check_call_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
            Ok(d.ret.clone())
        }
        other => Err(type_error(
            format!(
                "type `{}` has no method `{name}`",
                types::render_type(other)
            ),
            fspan,
        )),
    }
}

fn check_struct_construction(
    s: &StructInfo,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    let self_ty = Type::Named(s.decl.name.clone(), vec![]);
    if let Some((ia, id)) = s.init() {
        check_call_args(&ia.params, &id.params, args, call_span, fctx, mctx)?;
        return match &id.ret {
            Type::Unit => Ok(self_ty),
            Type::Result(ok, err) if **ok == Type::Unit => {
                Ok(Type::Result(Box::new(self_ty), err.clone()))
            }
            _ => Err(unimplemented_at(
                "a non-standard init return type is",
                call_span,
            )),
        };
    }
    check_struct_literal(s, args, call_span, fctx, mctx)?;
    Ok(self_ty)
}

/// A struct without `init` builds from its named-field literal
/// (02-language.md §7.1): every field exactly once unless defaulted,
/// positional only for a one-field struct.
fn check_struct_literal(
    s: &StructInfo,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let fields: Vec<(String, Type, bool)> = s
        .members()
        .filter_map(|(am, dm)| match (am, dm) {
            (Member::Field(af), DeclMember::Field(df)) => {
                Some((af.name.clone(), df.ty.clone(), af.default.is_some()))
            }
            _ => None,
        })
        .collect();
    if fields.len() == 1 && args.len() == 1 && args[0].label.is_none() {
        check_expr(&args[0].value, Some(&fields[0].1), fctx, mctx)?;
        return Ok(());
    }
    let mut bound = vec![false; fields.len()];
    for a in args {
        let Some(label) = &a.label else {
            return Err(type_error(
                "struct construction requires labeled fields (positional only for a one-field struct)".to_string(),
                a.span,
            ));
        };
        let Some(idx) = fields.iter().position(|f| &f.0 == label) else {
            return Err(type_error(format!("unknown field `{label}`"), a.span));
        };
        if bound[idx] {
            return Err(type_error(
                format!("field `{label}` supplied more than once"),
                a.span,
            ));
        }
        bound[idx] = true;
        let fty = fields[idx].1.clone();
        check_expr(&a.value, Some(&fty), fctx, mctx)?;
    }
    for (i, (name, _, has_default)) in fields.iter().enumerate() {
        if !bound[i] && !has_default {
            return Err(type_error(format!("missing field `{name}`"), call_span));
        }
    }
    Ok(())
}

/// Arity + label checking shared by fn/method/init calls
/// (02-language.md §5.1): each argument binds to a parameter either by
/// label (looked up by name) or positionally (the next not-yet-bound
/// parameter, left to right); every parameter must end up bound exactly
/// once, unless it has a default. Access-mode markers on `args` are
/// parsed but not validated here (item D's job).
fn check_call_args(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let mut bound = vec![false; decl_params.len()];
    let mut cursor = 0usize;
    for a in args {
        let idx = match &a.label {
            Some(lbl) => {
                let Some(i) = decl_params.iter().position(|p| &p.name == lbl) else {
                    return Err(type_error(
                        format!("unknown parameter label `{lbl}`"),
                        a.span,
                    ));
                };
                i
            }
            None => {
                while cursor < bound.len() && bound[cursor] {
                    cursor += 1;
                }
                if cursor >= decl_params.len() {
                    return Err(type_error("too many arguments".to_string(), a.span));
                }
                let i = cursor;
                cursor += 1;
                i
            }
        };
        if bound[idx] {
            return Err(type_error(
                format!("argument `{}` bound more than once", decl_params[idx].name),
                a.span,
            ));
        }
        bound[idx] = true;
        let pty = decl_params[idx].ty.clone();
        check_expr(&a.value, Some(&pty), fctx, mctx)?;
    }
    for (i, p) in decl_params.iter().enumerate() {
        if !bound[i] && ast_params[i].default.is_none() {
            return Err(type_error(
                format!("missing argument for parameter `{}`", p.name),
                call_span,
            ));
        }
    }
    Ok(())
}

/// Positional-only arg checking against a raw `fn(...)`-typed value
/// (a closure/named-function reference): unlike a real call, there are
/// no parameter names to label against.
fn check_positional_args(
    params: &[(AccessMode, Type)],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    if args.len() != params.len() {
        return Err(arity_error(params.len(), args.len(), call_span));
    }
    for (a, (_mode, ty)) in args.iter().zip(params.iter()) {
        if a.label.is_some() {
            return Err(type_error(
                "labeled arguments require a named function".to_string(),
                a.span,
            ));
        }
        check_expr(&a.value, Some(ty), fctx, mctx)?;
    }
    Ok(())
}

/// Enum variant construction (`Enum.Variant(...)`, leading-dot
/// `.Variant(...)`): positional only, mirroring the ast's own note that
/// pattern payloads "bind positionally regardless of whether the variant
/// was declared with named fields" (02-language.md §7.2).
fn check_variant_args(
    payload: &[Type],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    if args.len() != payload.len() {
        return Err(arity_error(payload.len(), args.len(), call_span));
    }
    for (a, ty) in args.iter().zip(payload.iter()) {
        check_expr(&a.value, Some(ty), fctx, mctx)?;
    }
    Ok(())
}

// --- the fail-closed set: defer's own `await`/`?` scan --------------------

/// `defer`'s body cannot `await` and cannot use `?` (02-language.md §10:
/// "a deferred action cannot await and cannot fail recoverably") — a
/// structural pre-scan over the raw ast, so this rejects with
/// `error[type]` before generic statement-checking ever reaches either
/// construct in the defer body (where `await` would otherwise be
/// `error[unimplemented]` and `?` would be checked normally).
fn scan_defer_forbidden(body: &DeferBody) -> Option<(&'static str, Span)> {
    match body {
        DeferBody::Expr(e) => scan_expr_forbidden(e),
        DeferBody::Suite(stmts) => scan_stmts_forbidden(stmts),
    }
}

fn scan_stmts_forbidden(stmts: &[Stmt]) -> Option<(&'static str, Span)> {
    stmts.iter().find_map(scan_stmt_forbidden)
}

fn scan_stmt_forbidden(s: &Stmt) -> Option<(&'static str, Span)> {
    match s {
        Stmt::Assign(a) => scan_expr_forbidden(&a.target).or_else(|| scan_expr_forbidden(&a.value)),
        Stmt::If(i) => scan_expr_forbidden(&i.cond)
            .or_else(|| scan_stmts_forbidden(&i.then_branch))
            .or_else(|| {
                i.elifs.iter().find_map(|e| {
                    scan_expr_forbidden(&e.cond).or_else(|| scan_stmts_forbidden(&e.body))
                })
            })
            .or_else(|| i.else_branch.as_ref().and_then(|b| scan_stmts_forbidden(b))),
        Stmt::Match(m) => scan_expr_forbidden(&m.scrutinee).or_else(|| {
            m.arms.iter().find_map(|a| {
                a.guard
                    .as_ref()
                    .and_then(scan_expr_forbidden)
                    .or_else(|| scan_stmts_forbidden(&a.body))
            })
        }),
        Stmt::For(f) => scan_expr_forbidden(&f.iterable).or_else(|| scan_stmts_forbidden(&f.body)),
        Stmt::While(w) => scan_expr_forbidden(&w.cond).or_else(|| scan_stmts_forbidden(&w.body)),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => None,
        Stmt::Return(_, e) => e.as_ref().and_then(scan_expr_forbidden),
        Stmt::Assert(a) => scan_expr_forbidden(&a.cond)
            .or_else(|| a.message.as_ref().and_then(scan_expr_forbidden)),
        Stmt::Defer(d) => scan_defer_forbidden(&d.body),
        Stmt::With(w) => scan_expr_forbidden(&w.expr).or_else(|| scan_stmts_forbidden(&w.body)),
        Stmt::Send(_, e) => scan_expr_forbidden(e),
        Stmt::Expr(_, e) => scan_expr_forbidden(e),
        Stmt::ComptimeIf(c) => scan_expr_forbidden(&c.cond)
            .or_else(|| scan_stmts_forbidden(&c.then_branch))
            .or_else(|| c.else_branch.as_ref().and_then(|b| scan_stmts_forbidden(b))),
        Stmt::ComptimeAssert(_, e, m) => {
            scan_expr_forbidden(e).or_else(|| m.as_ref().and_then(scan_expr_forbidden))
        }
    }
}

fn scan_expr_forbidden(e: &Expr) -> Option<(&'static str, Span)> {
    match e {
        Expr::Unary(span, UnaryOp::Await, _) => Some(("await", *span)),
        Expr::Try(span, _) => Some(("use `?`", *span)),
        Expr::Unary(_, _, inner) => scan_expr_forbidden(inner),
        Expr::Field(base, _, _) => scan_expr_forbidden(base),
        Expr::Index(base, _, args) => {
            scan_expr_forbidden(base).or_else(|| args.iter().find_map(scan_expr_forbidden))
        }
        Expr::Call(callee, _, args) => scan_expr_forbidden(callee)
            .or_else(|| args.iter().find_map(|a| scan_expr_forbidden(&a.value))),
        Expr::Binary(_, _, l, r) => scan_expr_forbidden(l).or_else(|| scan_expr_forbidden(r)),
        Expr::Range(_, a, b, _) => scan_expr_forbidden(a).or_else(|| scan_expr_forbidden(b)),
        Expr::Is(_, s, _) => scan_expr_forbidden(s),
        Expr::Not(_, i) => scan_expr_forbidden(i),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_expr_forbidden(l).or_else(|| scan_expr_forbidden(r))
        }
        Expr::DotVariant(_, _, args) => args.iter().find_map(|a| scan_expr_forbidden(&a.value)),
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => scan_expr_forbidden(e),
            ClosureBody::Suite(s) => scan_stmts_forbidden(s),
        },
        Expr::Send(_, i) => scan_expr_forbidden(i),
        Expr::Tuple(_, items) | Expr::List(_, items) => items.iter().find_map(scan_expr_forbidden),
        _ => None,
    }
}

// --- shared error helpers --------------------------------------------------

fn type_error(message: String, span: Span) -> SemaError {
    SemaError::at("type", message, span)
}

fn arity_error(expected: usize, found: usize, span: Span) -> SemaError {
    type_error(
        format!("expected {expected} argument(s), found {found}"),
        span,
    )
}
