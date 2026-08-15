use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, PixelsFnKind, PixelsFnMeta, TestDecl, TestKind, TypedCallArg, TypedClosureBody,
    TypedClosureParam, TypedConst, TypedDeferBody, TypedElif, TypedEnum, TypedExpr, TypedExprKind,
    TypedFn, TypedForIter, TypedMatchArm, TypedParam, TypedPattern, TypedPatternKind, TypedProgram,
    TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::{
    self, Classification, DeclMember, DeclParam, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssertStmt, AssignOp, AssignStmt, BinOp, ClosureBody, ClosureExpr,
    DeferBody, DeferStmt, Expr, ForStmt, IfStmt, Item, MatchArm, MatchStmt, Member, Module,
    NamedType, Pattern, Span, Stmt, UnaryOp, VariantPayload, WhileStmt,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum InstKind {
    Struct,
    Enum,
    Fn,
    Method,
}

impl InstKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            InstKind::Struct => "struct",
            InstKind::Enum => "enum",
            InstKind::Fn => "fn",
            InstKind::Method => "method",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueuedInstantiation {
    pub(crate) kind: InstKind,
    pub(crate) name: String,
    pub(crate) args: Vec<types::TypeArg>,
    pub(crate) receiver: Option<Type>,
    pub(crate) chain: Vec<Span>,
}

pub(crate) const MAX_GENERIC_DEPTH: usize = 64;

#[derive(Clone)]
pub(crate) struct StructInfo {
    pub(crate) decl: types::DeclStruct,
    pub(crate) ast_members: Vec<Member>,
    pub(crate) deferred_comptime_members: Vec<Member>,
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

    pub(crate) fn field_is_pub(&self, name: &str) -> Option<bool> {
        self.decl.members.iter().find_map(|m| match m {
            DeclMember::Field(d) if d.name == name => Some(d.is_pub),
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

#[derive(Clone)]
pub(crate) struct EnumInfo {
    pub(crate) decl: types::DeclEnum,
    pub(crate) ast_members: Vec<Member>,
}

impl EnumInfo {
    pub(crate) fn members(&self) -> impl Iterator<Item = (&Member, &DeclMember)> {
        self.ast_members.iter().zip(self.decl.members.iter())
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

    pub(crate) fn has_member_named(&self, name: &str) -> bool {
        self.ast_members.iter().any(|m| match m {
            Member::Fn(f) => f.name == name,
            _ => false,
        })
    }
}

impl std::ops::Deref for EnumInfo {
    type Target = types::DeclEnum;
    fn deref(&self) -> &Self::Target {
        &self.decl
    }
}

impl std::ops::DerefMut for EnumInfo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.decl
    }
}

#[derive(Clone)]
pub(crate) struct FnInfo {
    pub(crate) ast: ast::FnItem,
    pub(crate) decl: types::DeclFn,
}

pub(crate) struct ModuleCtx {
    pub(crate) shapes: BTreeMap<String, usize>,
    pub(crate) module_pools: BTreeSet<String>,
    pub(crate) structs: BTreeMap<String, StructInfo>,
    pub(crate) enums: BTreeMap<String, EnumInfo>,
    pub(crate) fns: BTreeMap<String, FnInfo>,
    pub(crate) consts: BTreeMap<String, Type>,
    pub(crate) statics: BTreeMap<String, StaticInfo>,
    pub(crate) layouts: BTreeMap<String, types::LayoutType>,
    pub(crate) const_values: BTreeMap<String, Expr>,
    pub(crate) generics_queue: RefCell<BTreeMap<String, QueuedInstantiation>>,
    pub(crate) current_chain: RefCell<Vec<Span>>,
    pub(crate) virtqueue_configures: RefCell<Vec<(String, u16)>>,
    pub(crate) reserve_permit_demands: RefCell<Vec<Span>>,
    pub(crate) unbounded_sync_loops: RefCell<Vec<crate::sema::typed::UnboundedSyncLoop>>,
    pub(crate) inferred_rets: RefCell<BTreeMap<String, Type>>,
    pub(crate) pixels_fn_meta: RefCell<BTreeMap<String, PixelsFnMeta>>,
    pub(crate) module_path: String,
    pub(crate) loader_key: Vec<String>,
    pub(crate) struct_decl_module: BTreeMap<String, String>,
    pub(crate) type_decl_module: BTreeMap<String, String>,
    pub(crate) type_decl_name: BTreeMap<String, String>,
    pub(crate) const_decl_module: BTreeMap<String, String>,
    pub(crate) const_decl_name: BTreeMap<String, String>,
    pub(crate) fn_decl_module: BTreeMap<String, String>,
    pub(crate) fn_decl_name: BTreeMap<String, String>,
    pub(crate) visibility_home: RefCell<Option<String>>,
}

#[derive(Debug, Clone)]
pub(crate) struct StaticInfo {
    pub(crate) ty: Type,
}

impl ModuleCtx {
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

pub(crate) fn build_module_ctx(
    module: &Module,
    decl_items: &[types::DeclItem],
    imported: &types::ImportedTypes,
) -> ModuleCtx {
    let module_path = module.path.join(".");
    let mut shapes: BTreeMap<String, usize> = imported.clone();
    let mut module_pools = BTreeSet::new();
    let mut structs = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut consts = BTreeMap::new();
    let mut statics = BTreeMap::new();
    let mut const_values = BTreeMap::new();
    let mut struct_decl_module = BTreeMap::new();
    let mut type_decl_module = BTreeMap::new();
    let mut type_decl_name = BTreeMap::new();
    let mut const_decl_module = BTreeMap::new();
    let mut const_decl_name = BTreeMap::new();
    let mut fn_decl_module = BTreeMap::new();
    let mut fn_decl_name = BTreeMap::new();

    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();

    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Struct(s), types::DeclItem::Struct(d)) => {
                shapes.insert(s.name.clone(), s.generics.len());
                let mut ast_members = Vec::new();
                let mut deferred_comptime_members = Vec::new();
                for m in &s.members {
                    match m {
                        Member::ComptimeIf(_) => deferred_comptime_members.push(m.clone()),
                        other => ast_members.push(other.clone()),
                    }
                }
                if s.deriving.iter().any(|d| d == "From") {
                    let field = s
                        .members
                        .iter()
                        .find_map(|m| match m {
                            Member::Field(f) => Some(f),
                            _ => None,
                        })
                        .expect("validate_from_shape already required exactly one field");
                    ast_members.push(Member::Fn(types::derived_from_fn_item_struct(
                        &s.name, field, s.span,
                    )));
                }
                if s.deriving.iter().any(|d| d == "Format") {
                    let fields: Vec<(String, Type)> = d
                        .members
                        .iter()
                        .filter_map(|m| match m {
                            types::DeclMember::Field(f) => Some((f.name.clone(), f.ty.clone())),
                            _ => None,
                        })
                        .collect();
                    let bound = types::struct_format_bound(&s.name, &fields, s.span)
                        .expect("declare already validated Format shape");
                    ast_members.push(Member::Fn(types::derived_max_formatted_len_fn_item(
                        bound, s.span,
                    )));
                    if fields.is_empty() {
                        ast_members.push(Member::Fn(types::derived_format_fn_item_struct(
                            &s.name, s.span,
                        )));
                    } else {
                        ast_members.push(Member::Fn(types::derived_format_fn_item_struct_fields(
                            &s.name, &fields, bound, s.span,
                        )));
                    }
                }
                struct_decl_module.insert(s.name.clone(), module_path.clone());
                type_decl_module.insert(s.name.clone(), module_path.clone());
                type_decl_name.insert(s.name.clone(), s.name.clone());
                structs.insert(
                    s.name.clone(),
                    StructInfo {
                        decl: d.clone(),
                        ast_members,
                        deferred_comptime_members,
                    },
                );
            }
            (Item::Enum(e), types::DeclItem::Enum(d)) => {
                shapes.insert(e.name.clone(), e.generics.len());
                type_decl_module.insert(e.name.clone(), module_path.clone());
                type_decl_name.insert(e.name.clone(), e.name.clone());
                let mut ast_members = e.members.clone();
                if e.deriving.iter().any(|d| d == "From") {
                    let v = &e.variants[0];
                    let source_ty = match &v.payload {
                        VariantPayload::Tuple(types) => &types[0],
                        VariantPayload::Named(fields) => &fields[0].ty,
                        VariantPayload::None => {
                            unreachable!("validate_from_shape already required exactly one field")
                        }
                    };
                    ast_members.push(Member::Fn(types::derived_from_fn_item_enum(
                        &e.name, &v.name, source_ty, e.span,
                    )));
                }
                if e.deriving.iter().any(|d| d == "Format") {
                    let bound = e.variants.iter().map(|v| v.name.len()).max().unwrap_or(0) as u64;
                    let names: Vec<String> = e.variants.iter().map(|v| v.name.clone()).collect();
                    ast_members.push(Member::Fn(types::derived_max_formatted_len_fn_item(
                        bound, e.span,
                    )));
                    ast_members.push(Member::Fn(types::derived_format_fn_item_enum(
                        &names, bound, e.span,
                    )));
                }
                enums.insert(
                    e.name.clone(),
                    EnumInfo {
                        decl: d.clone(),
                        ast_members,
                    },
                );
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => {
                fn_decl_module.insert(f.name.clone(), module_path.clone());
                fn_decl_name.insert(f.name.clone(), f.name.clone());
                fns.insert(
                    f.name.clone(),
                    FnInfo {
                        ast: f.clone(),
                        decl: d.clone(),
                    },
                );
            }
            (Item::Const(c), types::DeclItem::Const(d)) => {
                const_decl_module.insert(c.name.clone(), module_path.clone());
                const_decl_name.insert(c.name.clone(), c.name.clone());
                consts.insert(c.name.clone(), d.ty.clone());
                const_values.insert(c.name.clone(), c.value.clone());
            }
            (Item::Static(s), types::DeclItem::Static(d)) => {
                statics.insert(s.name.clone(), StaticInfo { ty: d.ty.clone() });
            }
            (Item::Pool(p), types::DeclItem::Pool(_)) => {
                module_pools.insert(p.name.clone());
            }
            _ => unreachable!("declare()'s items must pair 1:1 with the filtered ast items"),
        }
    }

    let layouts = types::check_layouts(module)
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    ModuleCtx {
        shapes,
        module_pools,
        structs,
        enums,
        fns,
        consts,
        statics,
        const_values,
        layouts,
        generics_queue: RefCell::new(BTreeMap::new()),
        current_chain: RefCell::new(Vec::new()),
        virtqueue_configures: RefCell::new(Vec::new()),
        reserve_permit_demands: RefCell::new(Vec::new()),
        unbounded_sync_loops: RefCell::new(Vec::new()),
        inferred_rets: RefCell::new(BTreeMap::new()),
        pixels_fn_meta: RefCell::new(BTreeMap::new()),
        module_path,
        loader_key: module.path.clone(),
        struct_decl_module,
        type_decl_module,
        type_decl_name,
        const_decl_module,
        const_decl_name,
        fn_decl_module,
        fn_decl_name,
        visibility_home: RefCell::new(None),
    }
}

pub(crate) fn enqueue_instantiation(
    mctx: &ModuleCtx,
    kind: InstKind,
    name: &str,
    args: &[types::TypeArg],
    call_span: Span,
) -> Result<String, SemaError> {
    debug_assert_ne!(
        kind,
        InstKind::Method,
        "method instantiations use enqueue_method_instantiation"
    );
    enqueue_instantiation_inner(mctx, kind, name, args, None, call_span)
}

pub(crate) fn enqueue_method_instantiation(
    mctx: &ModuleCtx,
    receiver: &Type,
    method: &str,
    args: &[types::TypeArg],
    call_span: Span,
) -> Result<String, SemaError> {
    enqueue_instantiation_inner(
        mctx,
        InstKind::Method,
        method,
        args,
        Some(receiver.clone()),
        call_span,
    )
}

fn enqueue_instantiation_inner(
    mctx: &ModuleCtx,
    kind: InstKind,
    name: &str,
    args: &[types::TypeArg],
    receiver: Option<Type>,
    call_span: Span,
) -> Result<String, SemaError> {
    let key = match (&kind, &receiver) {
        (InstKind::Method, Some(recv)) => generics::canonical_method_key(recv, name, args),
        _ => generics::canonical_key(kind, name, args),
    };
    let mut chain = mctx.current_chain.borrow().clone();
    chain.push(call_span);
    if chain.len() > MAX_GENERIC_DEPTH {
        return Err(SemaError::at(
            "generic",
            format!(
                "instantiation depth exceeded {MAX_GENERIC_DEPTH} while instantiating `{name}`"
            ),
            call_span,
        ));
    }
    mctx.generics_queue
        .borrow_mut()
        .entry(key.clone())
        .or_insert_with(|| QueuedInstantiation {
            kind,
            name: name.to_string(),
            args: args.to_vec(),
            receiver,
            chain,
        });
    Ok(key)
}

pub(crate) struct FnCtx {
    pub(crate) ret_ty: Type,
    locals: Vec<BTreeMap<String, Type>>,
    pub(crate) local_pools: BTreeSet<String>,
    pub(crate) group_children: BTreeMap<String, (Type, usize)>,
    pub(crate) in_async: bool,
    pub(crate) fn_name: String,
    unknown_outcome_arms: usize,
    pub(crate) quarantined_by_queue: BTreeMap<String, (String, String)>,
    inferred_errors: Option<Vec<Type>>,
}

impl FnCtx {
    pub(crate) fn new(ret_ty: Type, local_pools: BTreeSet<String>) -> FnCtx {
        let inferred_errors = if types::is_inferred_result(&ret_ty) {
            Some(Vec::new())
        } else {
            None
        };
        FnCtx {
            ret_ty,
            locals: vec![BTreeMap::new()],
            local_pools,
            group_children: BTreeMap::new(),
            in_async: false,
            fn_name: String::new(),
            unknown_outcome_arms: 0,
            quarantined_by_queue: BTreeMap::new(),
            inferred_errors,
        }
    }

    fn record_inferred_error(&mut self, ty: Type) {
        if self.inferred_errors.is_none() {
            return;
        }
        if types::is_inferred_error_set(&ty) {
            return;
        }
        if matches!(ty, Type::Never) {
            return;
        }
        if let Type::Named(n, args) = &ty {
            if n == types::ERROR_SET_NAME {
                for a in args {
                    if let types::TypeArg::Type(t) = a {
                        self.record_inferred_error(t.clone());
                    }
                }
                return;
            }
        }
        let Some(v) = &mut self.inferred_errors else {
            return;
        };
        if v.iter().any(|e| types_eq(e, &ty)) {
            return;
        }
        v.push(ty);
    }

    pub(crate) fn in_unknown_outcome_arm(&self) -> bool {
        self.unknown_outcome_arms > 0
    }

    pub(crate) fn push_scope(&mut self) {
        self.locals.push(BTreeMap::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.locals.pop();
    }

    pub(crate) fn lookup_local(&self, name: &str) -> Option<Type> {
        for scope in self.locals.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    pub(crate) fn retype_local(&mut self, name: &str, ty: Type) -> bool {
        for scope in self.locals.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), ty);
                return true;
            }
        }
        false
    }

    pub(crate) fn lookup_innermost(&self, name: &str) -> Option<Type> {
        self.locals
            .last()
            .expect("at least one scope")
            .get(name)
            .cloned()
    }

    pub(crate) fn insert_local(&mut self, name: String, ty: Type) {
        self.locals
            .last_mut()
            .expect("at least one scope")
            .insert(name, ty);
    }
}

pub(crate) fn scoped<T>(
    fctx: &mut FnCtx,
    f: impl FnOnce(&mut FnCtx) -> Result<T, SemaError>,
) -> Result<T, SemaError> {
    fctx.push_scope();
    let saved_quarantine = fctx.quarantined_by_queue.clone();
    let result = f(fctx);
    fctx.quarantined_by_queue = saved_quarantine;
    fctx.pop_scope();
    result
}

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

pub(crate) fn check(
    module: &Module,
    decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
) -> Result<TypedProgram, SemaError> {
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    let mut program = TypedProgram::default();
    program.module_path = mctx.module_path.clone();
    program.declared_pools = mctx.module_pools.clone();
    program.const_decl_modules = mctx.const_decl_module.clone();
    program.const_decl_names = mctx.const_decl_name.clone();
    program.fn_decl_modules = mctx.fn_decl_module.clone();
    program.fn_decl_names = mctx.fn_decl_name.clone();
    program.type_decl_modules = mctx.type_decl_module.clone();
    program.type_decl_names = mctx.type_decl_name.clone();
    for name in [
        "Target",
        "Transport",
        "Failure",
        "BootError",
        "DriverMode",
        "CompletionOutcome",
    ] {
        let variants = crate::sema::stdlib_enums::variant_strs(name)?
            .ok_or_else(|| {
                SemaError::at(
                    "build",
                    format!("stdlib enum `{name}` missing from the auto-visible table"),
                    Span::default(),
                )
            })?
            .iter()
            .map(|v| v.to_string())
            .collect();
        program
            .enums
            .insert(name.to_string(), TypedEnum::from_variants(variants));
    }
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Const(c), types::DeclItem::Const(d)) => {
                let mut fctx = FnCtx::new(Type::Unit, mctx.module_pools.clone());
                let value = check_expr(&c.value, Some(&d.ty), &mut fctx, mctx)?;
                program.consts.insert(
                    c.name.clone(),
                    TypedConst {
                        ty: d.ty.clone(),
                        value,
                    },
                );
            }
            (Item::Static(s), types::DeclItem::Static(d)) => {
                if contains_opaque_field(&d.ty, mctx) && mctx.module_path != "field" {
                    return Err(type_error(
                        format!(
                            "P004: opaque `Field` may not be stored in static `{}`",
                            d.name
                        ),
                        s.span,
                    ));
                }
                program.statics.insert(
                    d.name.clone(),
                    crate::sema::typed::TypedStatic {
                        ty: d.ty.clone(),
                        addr: d.addr,
                    },
                );
                program.declared_statics.insert(d.name.clone());
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => {
                check_marker_attr_shape(f, true)?;
                let mut pixels_meta = check_pixels_fn_shape(f, d, mctx)?;
                if f.is_pub
                    && contains_opaque_field(&d.ret, mctx)
                    && pixels_meta
                        .as_ref()
                        .is_none_or(|meta| meta.kind != PixelsFnKind::Field)
                    && mctx.module_path != "field"
                {
                    return Err(type_error(
                        format!(
                            "P004: public function `{}` may not return opaque `Field` without `@field`",
                            f.name
                        ),
                        f.span,
                    ));
                }
                let test_kind = test_attr_kind(f)?;
                if test_kind == Some(TestKind::Exhaustive) {
                    check_exhaustive_test_params(f, d, mctx)?;
                }
                if test_kind == Some(TestKind::Runtime) {
                    check_runtime_test_params(f, d)?;
                }
                check_layout_assert_fn(f, d, mctx)?;
                if let Some(tf) = check_top_fn(f, d, mctx)? {
                    if let Some(meta) = pixels_meta.as_mut() {
                        if meta.kind == PixelsFnKind::Field {
                            meta.material_type = pixels_field_material_type(&tf, mctx)?;
                        }
                    }
                    if let Some(meta) = pixels_meta {
                        mctx.pixels_fn_meta
                            .borrow_mut()
                            .insert(f.name.clone(), meta.clone());
                        program.pixels_fns.insert(f.name.clone(), meta);
                    }
                    if is_image_fn(f) {
                        if let Some(existing) = &program.image_fn {
                            return Err(SemaError::at(
                                "build",
                                format!(
                                    "more than one `@image` fn in this module (`{existing}` and `{}`)",
                                    f.name
                                ),
                                f.span,
                            ));
                        }
                        program.image_fn = Some(f.name.clone());
                    }
                    program.fns.insert(f.name.clone(), tf);
                    if let Some(kind) = test_kind {
                        program.tests.push(TestDecl {
                            name: f.name.clone(),
                            kind,
                        });
                    }
                } else if test_kind.is_some() {
                    return Err(unimplemented_at("`@test` on a generic fn is", f.span));
                }
            }
            (Item::Struct(s), types::DeclItem::Struct(d)) => {
                for m in &s.members {
                    if let ast::Member::Fn(mf) = m {
                        check_marker_attr_shape(mf, false)?;
                    }
                }
                if !s.generics.is_empty() && s.name != "Field" {
                    for (am, dm) in s.members.iter().zip(&d.members) {
                        if let (Member::Field(af), DeclMember::Field(df)) = (am, dm)
                            && contains_opaque_field(&df.ty, mctx)
                        {
                            return Err(type_error(
                                format!(
                                    "P004: opaque `Field` may not be stored in user struct \
                                     `{}.{}`",
                                    s.name, af.name
                                ),
                                af.span,
                            ));
                        }
                    }
                }
                if let Some(ts) = check_struct_bodies(s, mctx)? {
                    program.structs.insert(s.name.clone(), ts);
                }
            }
            (Item::Enum(e), types::DeclItem::Enum(_d)) => {
                for member in &e.members {
                    if let ast::Member::Fn(method) = member {
                        check_marker_attr_shape(method, false)?;
                    }
                }
                if let Some(te) = check_enum_bodies(e, mctx)? {
                    program.enums.insert(e.name.clone(), te);
                }
            }
            _ => {}
        }
    }
    program.virtqueue_configures = mctx.virtqueue_configures.borrow().clone();
    program.reserve_permit_demands = mctx.reserve_permit_demands.borrow().clone();
    program.unbounded_sync_loops = mctx.unbounded_sync_loops.borrow().clone();
    Ok(program)
}

pub(crate) fn check_marker_attr_shape(f: &ast::FnItem, top_level: bool) -> Result<(), SemaError> {
    let markers: Vec<&ast::Attr> = f
        .attrs
        .iter()
        .filter(|a| {
            a.name == "test"
                || a.name == "image"
                || a.name == "layout_assert"
                || a.name == "field"
                || a.name == "material"
        })
        .collect();
    if let Some(first) = markers.first() {
        if !top_level {
            let message = format!(
                "`@{}` is only valid on a top-level fn, not a struct member (`{}`)",
                first.name, f.name
            );
            return Err(match first.name.as_str() {
                "field" | "material" => SemaError::at(
                    "pixels P002",
                    format!(
                        "`@{}` function `{}` has unsupported parameter shape: {message}",
                        first.name, f.name
                    ),
                    first.span,
                ),
                _ => type_error(message, first.span),
            });
        }
        if markers.len() > 1 {
            let legacy_markers = ["test", "image", "layout_assert"];
            let marker_set = if legacy_markers.contains(&markers[0].name.as_str())
                && legacy_markers.contains(&markers[1].name.as_str())
            {
                "`@test`/`@image`/`@layout_assert` marker"
            } else {
                "top-level marker"
            };
            let pixels_category = if markers
                .iter()
                .any(|marker| matches!(marker.name.as_str(), "field" | "material"))
            {
                Some("pixels P002")
            } else {
                None
            };
            let detail = format!(
                "fn `{}` carries more than one {marker_set} attribute (`@{}` and `@{}`) — at most one is valid",
                f.name, markers[0].name, markers[1].name
            );
            return Err(match pixels_category {
                Some(category) => {
                    let marker = markers
                        .iter()
                        .find(|marker| matches!(marker.name.as_str(), "field" | "material"))
                        .expect("pixels marker selected category");
                    SemaError::at(
                        category,
                        format!(
                            "`@{}` function `{}` has unsupported parameter shape: {detail}",
                            marker.name, f.name
                        ),
                        markers[1].span,
                    )
                }
                None => type_error(detail, markers[1].span),
            });
        }
    }
    Ok(())
}

fn check_pixels_fn_shape(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<Option<PixelsFnMeta>, SemaError> {
    let pixels_error = |code: &'static str, message: String, span: Span| {
        let category = match code {
            "P001" => "pixels P001",
            "P002" => "pixels P002",
            _ => "pixels",
        };
        SemaError::at(category, message, span)
    };
    let named_from = |ty: &Type, name: &str, module: &str| {
        let Type::Named(found, _) = ty else {
            return false;
        };
        mctx.type_decl_name
            .get(found)
            .is_some_and(|declaration| declaration == name)
            && mctx
                .type_decl_module
                .get(found)
                .is_some_and(|origin| match module {
                    "field" => matches!(origin.as_str(), "field" | "core.field"),
                    "render" => matches!(origin.as_str(), "render" | "core.render"),
                    _ => origin == module,
                })
    };
    let field = f.attrs.iter().find(|attr| attr.name == "field");
    let material = f.attrs.iter().find(|attr| attr.name == "material");
    let Some((kind, attr)) = field
        .map(|attr| (PixelsFnKind::Field, attr))
        .or_else(|| material.map(|attr| (PixelsFnKind::Material, attr)))
    else {
        return Ok(None);
    };
    if !attr.args.is_empty() {
        return Err(pixels_error(
            "P002",
            format!(
                "`@{}` function `{}` has unsupported parameter shape: marker attributes take no arguments",
                attr.name, f.name
            ),
            attr.span,
        ));
    }
    if d.is_async || d.is_task || !d.generics.is_empty() || d.receiver.is_some() {
        return Err(pixels_error(
            "P002",
            format!(
                "`@{}` function `{}` has unsupported parameter shape: expected a top-level \
                 synchronous nongeneric function",
                attr.name, f.name,
            ),
            f.span,
        ));
    }
    match kind {
        PixelsFnKind::Field => {
            if !named_from(&d.ret, "Field", "field") {
                return Err(pixels_error(
                    "P001",
                    format!(
                        "`@field` function `{}` must return `Field`, found `{}`",
                        f.name,
                        types::render_type(&d.ret)
                    ),
                    f.span,
                ));
            }
            if !(d.params.len() == 1 || d.params.len() == 2)
                || d.params[0].mode != AccessMode::Read
                || !named_from(&d.params[0].ty, "Vec3", "field")
                || d.params.get(1).is_some_and(|p| p.mode != AccessMode::Read)
                || d.params
                    .get(1)
                    .is_some_and(|p| is_resource_type(&p.ty, mctx))
            {
                return Err(pixels_error(
                    "P002",
                    format!(
                        "`@field` function `{}` has unsupported parameter shape; expected \
                         `(p: Vec3)` or `(p: Vec3, read params: P)`",
                        f.name
                    ),
                    f.span,
                ));
            }
            Ok(Some(PixelsFnMeta {
                kind,
                params_type: d.params.get(1).map(|p| p.ty.clone()),
                material_type: None,
            }))
        }
        PixelsFnKind::Material => {
            if !named_from(&d.ret, "MaterialSample", "render") {
                return Err(pixels_error(
                    "P001",
                    format!(
                        "`@material` function `{}` must return `MaterialSample`, found `{}`",
                        f.name,
                        types::render_type(&d.ret)
                    ),
                    f.span,
                ));
            }
            let surface_material = match d.params.first().map(|p| (&p.mode, &p.ty)) {
                Some((AccessMode::Read, ty @ Type::Named(_, args)))
                    if named_from(ty, "SurfaceContext", "render") =>
                {
                    match args.as_slice() {
                        [TypeArg::Type(material)] => Some(material.clone()),
                        _ => None,
                    }
                }
                _ => None,
            };
            if !(d.params.len() == 1 || d.params.len() == 2)
                || surface_material.is_none()
                || !d
                    .params
                    .first()
                    .is_some_and(|param| named_from(&param.ty, "SurfaceContext", "render"))
                || d.params.get(1).is_some_and(|p| p.mode != AccessMode::Read)
                || d.params
                    .get(1)
                    .is_some_and(|p| is_resource_type(&p.ty, mctx))
            {
                return Err(pixels_error(
                    "P002",
                    format!(
                        "`@material` function `{}` has unsupported parameter shape; expected \
                         `(surface: SurfaceContext[M])` or \
                         `(surface: SurfaceContext[M], read params: P)`",
                        f.name
                    ),
                    f.span,
                ));
            }
            Ok(Some(PixelsFnMeta {
                kind,
                params_type: d.params.get(1).map(|p| p.ty.clone()),
                material_type: surface_material,
            }))
        }
    }
}

fn pixels_field_material_type(f: &TypedFn, mctx: &ModuleCtx) -> Result<Option<Type>, SemaError> {
    fn visit_expr(
        expr: &TypedExpr,
        found: &mut Option<Type>,
        mctx: &ModuleCtx,
    ) -> Result<(), SemaError> {
        let visit_args = |args: &[TypedCallArg], found: &mut Option<Type>| {
            for arg in args {
                if let Some(value) = &arg.value {
                    visit_expr(value, found, mctx)?;
                }
            }
            Ok::<(), SemaError>(())
        };
        match &expr.kind {
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let spelling = callee.spelling();
                let visible_name = crate::pixels::call_base(&spelling);
                let canonical_mark = mctx
                    .fn_decl_module
                    .get(visible_name)
                    .zip(mctx.fn_decl_name.get(visible_name))
                    .is_some_and(|(module, declaration)| {
                        declaration == "mark" && matches!(module.as_str(), "field" | "core.field")
                    });
                if canonical_mark && args.len() >= 3 {
                    let material = args[2]
                        .value
                        .as_ref()
                        .map(|value| value.ty.clone())
                        .expect("ordinary call arguments carry values");
                    if found.as_ref().is_some_and(|prior| prior != &material) {
                        return Err(SemaError::at(
                            "pixels P009",
                            format!(
                                "renderer field/material parameter types disagree: `@field` uses more than one nominal material type (`{}` and `{}`)",
                                types::render_type(found.as_ref().unwrap()),
                                types::render_type(&material)
                            ),
                            expr.span,
                        ));
                    }
                    *found = Some(material);
                }
                if let Some(receiver) = receiver {
                    visit_expr(receiver, found, mctx)?;
                }
                visit_args(args, found)?;
            }
            TypedExprKind::CallValue(callee, args) => {
                visit_expr(callee, found, mctx)?;
                visit_args(args, found)?;
            }
            TypedExprKind::Field(base, _)
            | TypedExprKind::ToScalar(base)
            | TypedExprKind::Neg(base)
            | TypedExprKind::BitNot(base)
            | TypedExprKind::Take(base)
            | TypedExprKind::Try(base, _)
            | TypedExprKind::Not(base)
            | TypedExprKind::Panic(base)
            | TypedExprKind::Await(base)
            | TypedExprKind::Send(base) => visit_expr(base, found, mctx)?,
            TypedExprKind::Index(a, b)
            | TypedExprKind::Binary(_, a, b)
            | TypedExprKind::OpCall(_, a, b)
            | TypedExprKind::And(a, b)
            | TypedExprKind::Or(a, b) => {
                visit_expr(a, found, mctx)?;
                visit_expr(b, found, mctx)?;
            }
            TypedExprKind::Is(value, _) => visit_expr(value, found, mctx)?,
            TypedExprKind::EnumConstruct { args, .. } => visit_args(args, found)?,
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                for item in items {
                    visit_expr(item, found, mctx)?;
                }
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                for (_, value) in fields {
                    visit_expr(value, found, mctx)?;
                }
            }
            TypedExprKind::Intrinsic { receiver, args, .. } => {
                if let Some(receiver) = receiver {
                    visit_expr(receiver, found, mctx)?;
                }
                for (_, value) in args {
                    visit_expr(value, found, mctx)?;
                }
            }
            TypedExprKind::Closure { .. }
            | TypedExprKind::Int(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Unit
            | TypedExprKind::Local(_)
            | TypedExprKind::Const(_)
            | TypedExprKind::Static(_)
            | TypedExprKind::FnRef(_)
            | TypedExprKind::PoolName(_)
            | TypedExprKind::GroupChild(_) => {}
        }
        Ok(())
    }
    fn visit_stmts(
        stmts: &[TypedStmt],
        found: &mut Option<Type>,
        mctx: &ModuleCtx,
    ) -> Result<(), SemaError> {
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { value, .. } => visit_expr(value, found, mctx)?,
                TypedStmtKind::Assign { target, value } => {
                    visit_expr(target, found, mctx)?;
                    visit_expr(value, found, mctx)?;
                }
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    visit_expr(cond, found, mctx)?;
                    visit_stmts(then_branch, found, mctx)?;
                    for elif in elifs {
                        visit_expr(&elif.cond, found, mctx)?;
                        visit_stmts(&elif.body, found, mctx)?;
                    }
                    if let Some(branch) = else_branch {
                        visit_stmts(branch, found, mctx)?;
                    }
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    visit_expr(scrutinee, found, mctx)?;
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            visit_expr(guard, found, mctx)?;
                        }
                        visit_stmts(&arm.body, found, mctx)?;
                    }
                }
                TypedStmtKind::For { iter, body, .. } => {
                    match iter {
                        TypedForIter::Range(a, b, _) => {
                            visit_expr(a, found, mctx)?;
                            visit_expr(b, found, mctx)?;
                        }
                        TypedForIter::Expr(value) => visit_expr(value, found, mctx)?,
                    }
                    visit_stmts(body, found, mctx)?;
                }
                TypedStmtKind::While { cond, body, .. } => {
                    visit_expr(cond, found, mctx)?;
                    visit_stmts(body, found, mctx)?;
                }
                TypedStmtKind::Return(Some(value))
                | TypedStmtKind::ExprStmt(value)
                | TypedStmtKind::BareSend { expr: value, .. } => visit_expr(value, found, mctx)?,
                TypedStmtKind::Assert { cond, message }
                | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                    visit_expr(cond, found, mctx)?;
                    if let Some(message) = message {
                        visit_expr(message, found, mctx)?;
                    }
                }
                TypedStmtKind::Defer(TypedDeferBody::Expr(value)) => {
                    visit_expr(value, found, mctx)?
                }
                TypedStmtKind::Defer(TypedDeferBody::Suite(body))
                | TypedStmtKind::WithGroup { body, .. } => visit_stmts(body, found, mctx)?,
                TypedStmtKind::Break
                | TypedStmtKind::Continue
                | TypedStmtKind::Pass
                | TypedStmtKind::Return(None) => {}
            }
        }
        Ok(())
    }
    let mut found = None;
    visit_stmts(&f.body, &mut found, mctx)?;
    Ok(found)
}

pub(crate) fn is_image_fn(f: &ast::FnItem) -> bool {
    f.attrs.iter().any(|a| a.name == "image")
}

pub(crate) fn is_layout_assert_fn(f: &ast::FnItem) -> bool {
    f.attrs.iter().any(|a| a.name == "layout_assert")
}

pub(crate) fn check_layout_assert_fn(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let Some(attr) = f.attrs.iter().find(|a| a.name == "layout_assert") else {
        return Ok(());
    };
    if !attr.args.is_empty() {
        return Err(type_error(
            "`@layout_assert` takes no arguments".to_string(),
            attr.span,
        ));
    }
    if d.params.len() != 1 {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}` must take exactly one parameter (`report: ImageReport`)",
                f.name
            ),
            f.span,
        ));
    }
    let p = &d.params[0];
    if p.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter `{}` must be a plain (read) parameter",
                f.name, p.name
            ),
            f.span,
        ));
    }
    let Type::Named(type_name, args) = &p.ty else {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter must have type `ImageReport`, found `{}`",
                f.name,
                types::render_type(&p.ty)
            ),
            f.span,
        ));
    };
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter must have type `ImageReport`, found `{}`",
                f.name,
                types::render_type(&p.ty)
            ),
            f.span,
        ));
    }
    if !mctx_has_image_report(mctx, type_name) {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter type `{type_name}` is not the stdlib \
                 `ImageReport` (import it with `from core.image_report import ImageReport`)",
                f.name
            ),
            f.span,
        ));
    }
    if d.ret != Type::Unit {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}` must return `unit`, found `{}`",
                f.name,
                types::render_type(&d.ret)
            ),
            f.span,
        ));
    }
    Ok(())
}

fn mctx_has_image_report(mctx: &ModuleCtx, type_name: &str) -> bool {
    const FIELDS: &[&str] = &[
        "machine_revision",
        "entry",
        "pages_base",
        "pages_size",
        "stacks_base",
        "stacks_size",
        "code_base",
        "code_size",
    ];
    let Some(info) = mctx.structs.get(type_name) else {
        return false;
    };
    let names: BTreeSet<&str> = info
        .decl
        .members
        .iter()
        .filter_map(|m| match m {
            DeclMember::Field(f) => Some(f.name.as_str()),
            _ => None,
        })
        .collect();
    FIELDS.iter().all(|n| names.contains(n)) && names.len() == FIELDS.len()
}

pub(crate) fn test_attr_kind(f: &ast::FnItem) -> Result<Option<TestKind>, SemaError> {
    let Some(attr) = f.attrs.iter().find(|a| a.name == "test") else {
        return Ok(None);
    };
    let kind = match attr.args.as_slice() {
        [] => TestKind::Comptime,
        [arg] => match &arg.value {
            Expr::Name(_, name) if name == "runtime" && arg.label.is_none() => TestKind::Runtime,
            Expr::Name(_, name) if name == "exhaustive" && arg.label.is_none() => {
                TestKind::Exhaustive
            }
            _ => {
                return Err(type_error(
                    "`@test`'s only argument is the bare name `runtime` or `exhaustive`"
                        .to_string(),
                    attr.span,
                ));
            }
        },
        _ => {
            return Err(type_error(
                "`@test` takes at most one argument (`runtime` or `exhaustive`)".to_string(),
                attr.span,
            ));
        }
    };
    match kind {
        TestKind::Exhaustive if f.params.is_empty() => Err(type_error(
            format!(
                "`@test(exhaustive)` fn `{}` needs at least one parameter (the enumerated domain)",
                f.name
            ),
            f.span,
        )),
        TestKind::Comptime if !f.params.is_empty() => Err(type_error(
            format!("`@test` fn `{}` takes no arguments", f.name),
            f.span,
        )),
        _ => Ok(Some(kind)),
    }
}

fn check_exhaustive_test_params(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    for p in &d.params {
        if p.mode != AccessMode::Read {
            return Err(type_error(
                format!(
                    "`@test(exhaustive)` fn `{}`'s parameter `{}` must be a plain (read) parameter",
                    f.name, p.name
                ),
                f.span,
            ));
        }
        let enumerable = match &p.ty {
            Type::Bool | Type::U8 | Type::I8 => true,
            Type::Named(name, targs) if targs.is_empty() => match mctx.enums.get(name) {
                Some(en) => en
                    .variants
                    .iter()
                    .all(|v| matches!(v.payload, types::DeclVariantPayload::None)),
                None => false,
            },
            _ => false,
        };
        if !enumerable {
            return Err(type_error(
                format!(
                    "`@test(exhaustive)` fn `{}`'s parameter `{}` has no enumerable domain \
                     (supported: `bool`, `u8`, `i8`, a fieldless enum), found `{}`",
                    f.name,
                    p.name,
                    types::render_type(&p.ty)
                ),
                f.span,
            ));
        }
    }
    Ok(())
}

fn check_runtime_test_params(f: &ast::FnItem, d: &types::DeclFn) -> Result<(), SemaError> {
    for p in &d.params {
        if p.mode != AccessMode::Read {
            return Err(type_error(
                format!(
                    "`@test(runtime)` fn `{}`'s parameter `{}` must be a plain (read) `Actor[T]` \
                     handle",
                    f.name, p.name
                ),
                f.span,
            ));
        }
        let is_handle =
            matches!(&p.ty, Type::Named(name, targs) if name == "Actor" && targs.len() == 1);
        if !is_handle {
            return Err(type_error(
                format!(
                    "`@test(runtime)` fn `{}`'s parameter `{}` must be an `Actor[T]` handle, \
                     found `{}`",
                    f.name,
                    p.name,
                    types::render_type(&p.ty)
                ),
                f.span,
            ));
        }
    }
    Ok(())
}

pub(crate) fn local_pool_names(info: &StructInfo) -> BTreeSet<String> {
    info.ast_members
        .iter()
        .filter_map(|m| match m {
            Member::Pool(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect()
}

pub(crate) fn check_top_fn(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<Option<TypedFn>, SemaError> {
    if is_image_fn(f) {
        if !f.generics.is_empty() {
            return Err(unimplemented_at("a generic `@image` fn is", f.span));
        }
        if d.ret != Type::Named("Image".to_string(), vec![]) {
            return Err(type_error(
                format!(
                    "`@image` fn `{}` must return `Image`, found `{}`",
                    f.name,
                    types::render_type(&d.ret)
                ),
                f.span,
            ));
        }
    }
    if !f.generics.is_empty() {
        return Ok(None);
    }
    let mut fctx = FnCtx::new(d.ret.clone(), mctx.module_pools.clone());
    fctx.in_async = f.is_async;
    fctx.fn_name = f.name.clone();
    let params = check_params_with_defaults(&f.params, &d.params, &mut fctx, mctx)?;
    let body = match &f.body {
        Some(body) => check_stmts(body, &mut fctx, mctx)?,
        None => return Err(unimplemented_at("bodyless functions are", f.span)),
    };
    if f.is_async {
        check_cross_await(&body)?;
    }
    if d.is_task {
        return Err(type_error(
            format!(
                "`@task` is only valid on a `@driver` method (03-hardware.md §6's bottom half); \
                 top-level fn `{}` cannot carry it",
                f.name
            ),
            f.span,
        ));
    }
    let ret = finalize_inferred_ret(&d.ret, fctx.inferred_errors, &f.name, None, mctx);
    Ok(Some(TypedFn {
        receiver: None,
        params,
        ret,
        body,
        is_async: f.is_async,
        is_task: false,
        is_layout_assert: is_layout_assert_fn(f),
        is_pub: f.is_pub,
    }))
}

fn finalize_inferred_ret(
    declared: &Type,
    inferred_errors: Option<Vec<Type>>,
    fn_name: &str,
    owner: Option<&str>,
    mctx: &ModuleCtx,
) -> Type {
    let Some(errs) = inferred_errors else {
        return declared.clone();
    };
    let Type::Result(ok, err) = declared else {
        return declared.clone();
    };
    if !types::is_inferred_error_set(err) {
        return declared.clone();
    }
    let err_ty = types::finalize_error_set(errs);
    let ret = Type::Result(ok.clone(), Box::new(err_ty));
    let key = inferred_ret_key(owner, fn_name);
    mctx.inferred_rets.borrow_mut().insert(key, ret.clone());
    ret
}

fn inferred_ret_key(owner: Option<&str>, fn_name: &str) -> String {
    match owner {
        Some(o) => format!("{o}.{fn_name}"),
        None => fn_name.to_string(),
    }
}

fn resolved_ret(declared: &Type, owner: Option<&str>, fn_name: &str, mctx: &ModuleCtx) -> Type {
    mctx.inferred_rets
        .borrow()
        .get(&inferred_ret_key(owner, fn_name))
        .cloned()
        .unwrap_or_else(|| declared.clone())
}

pub(crate) fn check_params_with_defaults(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedParam>, SemaError> {
    let mut out = Vec::with_capacity(decl_params.len());
    for (ap, dp) in ast_params.iter().zip(decl_params.iter()) {
        fctx.insert_local(dp.name.clone(), dp.ty.clone());
        let default = match &ap.default {
            Some(def) => Some(check_expr(def, Some(&dp.ty), fctx, mctx)?),
            None => None,
        };
        out.push(TypedParam {
            mode: dp.mode,
            name: dp.name.clone(),
            ty: dp.ty.clone(),
            default,
        });
    }
    Ok(out)
}

fn check_struct_bodies(
    s: &ast::StructItem,
    mctx: &ModuleCtx,
) -> Result<Option<TypedStruct>, SemaError> {
    if s.name == "Renderer" && !matches!(mctx.module_path.as_str(), "render" | "core.render") {
        return Err(type_error(
            "`Renderer` is reserved for the canonical `core.render::Renderer` actor".to_string(),
            s.span,
        ));
    }
    if !s.generics.is_empty() {
        return Ok(None);
    }
    let info = mctx.structs.get(&s.name).expect("struct present in mctx");
    let self_ty = Type::Named(s.name.clone(), vec![]);
    Ok(Some(check_struct_members(info, self_ty, mctx)?))
}

fn check_enum_bodies(e: &ast::EnumItem, mctx: &ModuleCtx) -> Result<Option<TypedEnum>, SemaError> {
    if e.name == "Renderer" && !matches!(mctx.module_path.as_str(), "render" | "core.render") {
        return Err(type_error(
            "`Renderer` is reserved for the canonical `core.render::Renderer` actor".to_string(),
            e.span,
        ));
    }
    let info = mctx.enums.get(&e.name).expect("enum present in mctx");
    for (variant, declaration) in e.variants.iter().zip(&info.variants) {
        if decl_variant_payload_types(declaration)
            .iter()
            .any(|ty| contains_opaque_field(ty, mctx))
        {
            return Err(type_error(
                format!(
                    "P004: opaque `Field` may not be stored in enum payload `{}.{}`",
                    e.name, variant.name
                ),
                variant.span,
            ));
        }
    }
    let variant_payload_types = info
        .variants
        .iter()
        .map(|v| match &v.payload {
            DeclVariantPayload::None => Vec::new(),
            DeclVariantPayload::Tuple(tys) => tys.clone(),
            DeclVariantPayload::Named(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
        })
        .collect();
    let generic_type_params = info
        .decl
        .generics
        .iter()
        .map(|generic| match generic.kind {
            crate::sema::types::DeclGenericKind::Type => Some(generic.name.clone()),
            crate::sema::types::DeclGenericKind::Const(_) => None,
        })
        .collect();
    if !e.generics.is_empty() {
        return Ok(Some(TypedEnum {
            variants: info.variants.iter().map(|v| v.name.clone()).collect(),
            variant_payload_types,
            generic_type_params,
            methods: BTreeMap::new(),
            assoc_fns: BTreeMap::new(),
        }));
    }
    let self_ty = Type::Named(e.name.clone(), vec![]);
    let mut methods = BTreeMap::new();
    let mut assoc_fns = BTreeMap::new();
    for (am, dm) in info.members() {
        let (Member::Fn(f), DeclMember::Fn(fd)) = (am, dm) else {
            continue;
        };
        if f.is_pub && contains_opaque_field(&fd.ret, mctx) && mctx.module_path != "field" {
            return Err(type_error(
                format!(
                    "P004: public function `{}.{}` may not return opaque `Field`; \
                     renderer roots must be top-level `@field` functions",
                    e.name, f.name
                ),
                f.span,
            ));
        }
        if !f.generics.is_empty() {
            continue;
        }
        if f.is_async && f.receiver.is_none() {
            return Err(unimplemented_at(
                "an `async fn` with no receiver (associated fn) is",
                f.span,
            ));
        }
        let mut fctx = FnCtx::new(fd.ret.clone(), mctx.module_pools.clone());
        fctx.in_async = f.is_async;
        fctx.fn_name = f.name.clone();
        fctx.insert_local("self".to_string(), self_ty.clone());
        let params = check_params_with_defaults(&f.params, &fd.params, &mut fctx, mctx)?;
        let body = match &f.body {
            Some(body) => check_stmts(body, &mut fctx, mctx)?,
            None => return Err(unimplemented_at("bodyless functions are", f.span)),
        };
        if f.is_async {
            check_cross_await(&body)?;
        }
        if fd.is_task {
            return Err(type_error(
                format!(
                    "`@task` is only valid on a `@driver` method (03-hardware.md §6); \
                     `{}.{}` is an enum method",
                    e.name, f.name
                ),
                f.span,
            ));
        }
        let receiver = f
            .receiver
            .as_ref()
            .map(|r| (r.mode.unwrap_or(AccessMode::Read), self_ty.clone()));
        let ret =
            finalize_inferred_ret(&fd.ret, fctx.inferred_errors, &f.name, Some(&e.name), mctx);
        let tf = TypedFn {
            receiver,
            params,
            ret,
            body,
            is_async: f.is_async,
            is_task: fd.is_task,
            is_layout_assert: false,
            is_pub: f.is_pub,
        };
        if f.receiver.is_some() {
            methods.insert(f.name.clone(), tf);
        } else {
            assoc_fns.insert(f.name.clone(), tf);
        }
    }
    validate_format_contract(&e.name, &methods, &assoc_fns, e.span)?;
    Ok(Some(TypedEnum {
        variants: info.variants.iter().map(|v| v.name.clone()).collect(),
        variant_payload_types,
        generic_type_params,
        methods,
        assoc_fns,
    }))
}

pub(crate) fn check_struct_members(
    info: &StructInfo,
    self_ty: Type,
    mctx: &ModuleCtx,
) -> Result<TypedStruct, SemaError> {
    let struct_name = match &self_ty {
        Type::Named(name, _) => name.clone(),
        other => unreachable!("check_struct_members: self_ty `{other:?}` is not Type::Named"),
    };
    let local_pools = local_pool_names(info);
    let mut fields = Vec::new();
    let mut field_types = BTreeMap::new();
    let mut field_contracts = BTreeMap::new();
    let mut field_defaults = BTreeMap::new();
    let mut methods = BTreeMap::new();
    let mut assoc_fns = BTreeMap::new();
    let mut init = None;
    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Field(af), DeclMember::Field(df)) => {
                if !is_opaque_field_name(&struct_name, mctx) && contains_opaque_field(&df.ty, mctx)
                {
                    return Err(type_error(
                        format!(
                            "P004: opaque `Field` may not be stored in user struct \
                             `{struct_name}.{}`",
                            af.name
                        ),
                        af.span,
                    ));
                }
                let field_index = fields.len();
                fields.push(af.name.clone());
                field_types.insert(af.name.clone(), df.ty.clone());
                let contracts = crate::sema::attrs::parse_field_contracts(
                    af,
                    &df.ty,
                    &mctx.const_values,
                    is_core_field_vector_type(&df.ty, mctx),
                )
                .map_err(|mut error| {
                    let path = format!("[{field_index}]");
                    match error.category {
                        "pixels P006"
                            if !error.message.starts_with(&format!("rate for {path} ")) =>
                        {
                            error.message = format!(
                                "rate for {path} is negative, non-finite, or not representable: {}",
                                error.message
                            );
                        }
                        "pixels P007"
                            if !error.message.starts_with(&format!("range for {path} ")) =>
                        {
                            error.message = format!(
                                "range for {path} is empty, non-finite, or not representable: {}",
                                error.message
                            );
                        }
                        _ => {}
                    }
                    error
                })?;
                field_contracts.insert(vec![field_index], contracts);
                if let Some(def) = &af.default {
                    let mut fctx = FnCtx::new(Type::Unit, local_pools.clone());
                    fctx.insert_local("self".to_string(), self_ty.clone());
                    let typed_def = check_expr(def, Some(&df.ty), &mut fctx, mctx)?;
                    field_defaults.insert(af.name.clone(), typed_def);
                }
            }
            (Member::Fn(f), DeclMember::Fn(fd)) => {
                if f.is_pub && contains_opaque_field(&fd.ret, mctx) && mctx.module_path != "field" {
                    return Err(type_error(
                        format!(
                            "P004: public function `{struct_name}.{}` may not return opaque `Field`; \
                             renderer roots must be top-level `@field` functions",
                            f.name
                        ),
                        f.span,
                    ));
                }
                if !f.generics.is_empty() {
                    continue;
                }
                if f.is_async && f.receiver.is_none() {
                    return Err(unimplemented_at(
                        "an `async fn` with no receiver (associated fn) is",
                        f.span,
                    ));
                }
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.in_async = f.is_async;
                fctx.fn_name = f.name.clone();
                fctx.insert_local("self".to_string(), self_ty.clone());
                let params = check_params_with_defaults(&f.params, &fd.params, &mut fctx, mctx)?;
                let body = match &f.body {
                    Some(body) => check_stmts(body, &mut fctx, mctx)?,
                    None => return Err(unimplemented_at("bodyless functions are", f.span)),
                };
                if f.is_async {
                    check_cross_await(&body)?;
                }
                if fd.is_task {
                    if !info.decl.is_driver {
                        return Err(type_error(
                            format!(
                                "`@task` is only valid on a `@driver` method (03-hardware.md §6); \
                                 `{struct_name}` is not a `@driver`"
                            ),
                            f.span,
                        ));
                    }
                    if f.is_async {
                        return Err(type_error(
                            format!(
                                "`@task` `{struct_name}.{}` must be a plain `fn`, not `async fn` \
                                 (03-hardware.md §6: the bottom half never stays active while \
                                 waiting)",
                                f.name
                            ),
                            f.span,
                        ));
                    }
                    if f.receiver.is_none() {
                        return Err(type_error(
                            format!(
                                "`@task` `{struct_name}.{}` must be a method with a `self` receiver",
                                f.name
                            ),
                            f.span,
                        ));
                    }
                }
                let receiver = f
                    .receiver
                    .as_ref()
                    .map(|r| (r.mode.unwrap_or(AccessMode::Read), self_ty.clone()));
                let ret = finalize_inferred_ret(
                    &fd.ret,
                    fctx.inferred_errors,
                    &f.name,
                    Some(&struct_name),
                    mctx,
                );
                let tf = TypedFn {
                    receiver,
                    params,
                    ret,
                    body,
                    is_async: f.is_async,
                    is_task: fd.is_task,
                    is_layout_assert: false,
                    is_pub: f.is_pub,
                };
                if f.receiver.is_some() {
                    methods.insert(f.name.clone(), tf);
                } else {
                    assoc_fns.insert(f.name.clone(), tf);
                }
            }
            (Member::Init(i), DeclMember::Init(fd)) => {
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                let params = check_params_with_defaults(&i.params, &fd.params, &mut fctx, mctx)?;
                let body = check_stmts(&i.body, &mut fctx, mctx)?;
                let ret = finalize_inferred_ret(
                    &fd.ret,
                    fctx.inferred_errors,
                    "init",
                    Some(&struct_name),
                    mctx,
                );
                init = Some(TypedFn {
                    receiver: Some((i.receiver.mode.unwrap_or(AccessMode::Mut), self_ty.clone())),
                    params,
                    ret,
                    body,
                    is_async: false,
                    is_task: false,
                    is_layout_assert: false,
                    is_pub: false,
                });
            }
            _ => {}
        }
    }
    validate_format_contract(&struct_name, &methods, &assoc_fns, info.decl.span)?;
    Ok(TypedStruct {
        name: struct_name,
        fields,
        field_types,
        field_contracts,
        field_defaults,
        methods,
        assoc_fns,
        init,
        is_resource: info.decl.classification == Classification::Resource,
        is_actor: info.decl.is_actor,
        is_driver: info.decl.is_driver,
    })
}

fn validate_format_contract(
    type_name: &str,
    methods: &BTreeMap<String, TypedFn>,
    assoc_fns: &BTreeMap<String, TypedFn>,
    span: Span,
) -> Result<(), SemaError> {
    let Some(max_fn) = assoc_fns.get("max_formatted_len") else {
        return Ok(());
    };
    let Some(fmt_fn) = methods.get("format") else {
        return Ok(());
    };
    if !typed_is_format_max(max_fn) || !typed_is_format_writer(fmt_fn) {
        return Ok(());
    }
    if type_name == "Secret" {
        return Err(types::secret_has_no_format(span));
    }
    let bound = format_bound_from_body(&max_fn.body, span)?;
    if !string_capacity_fits(i128::from(bound)) {
        return Err(type_error(
            format!("Format max_formatted_len bound {bound} is out of range for `String[..N]`"),
            span,
        ));
    }
    let Type::String(n_expr) = &fmt_fn.ret else {
        return Err(type_error(
            "Format.format must return `String[..N]`".to_string(),
            span,
        ));
    };
    let ret_n = literal_array_len(n_expr).ok_or_else(|| {
        type_error(
            "Format.format return capacity must be a literal".to_string(),
            span,
        )
    })?;
    let ret_n = u64::try_from(ret_n).map_err(|_| {
        type_error(
            "Format.format return capacity is out of range".to_string(),
            span,
        )
    })?;
    if ret_n != bound {
        return Err(type_error(
            format!("Format.format returns `String[..{ret_n}]` but max_formatted_len is {bound}"),
            span,
        ));
    }
    check_format_writer_against_bound(&fmt_fn.body, bound, span)
}

fn typed_is_format_max(f: &TypedFn) -> bool {
    f.receiver.is_none() && f.params.is_empty() && f.ret == Type::Usize && !f.is_async
}

fn typed_is_format_writer(f: &TypedFn) -> bool {
    matches!(&f.receiver, Some((AccessMode::Read, _)))
        && f.params.is_empty()
        && matches!(f.ret, Type::String(_))
        && !f.is_async
}

fn format_bound_from_body(body: &[TypedStmt], span: Span) -> Result<u64, SemaError> {
    let mut bound: Option<u64> = None;
    collect_format_bound_returns(body, span, &mut |v| {
        match bound {
            None => bound = Some(v),
            Some(b) if b == v => {}
            Some(b) => {
                return Err(type_error(
                    format!("Format max_formatted_len returns disagreeing bounds ({b} vs {v})"),
                    span,
                ));
            }
        }
        Ok(())
    })?;
    bound.ok_or_else(|| {
        type_error(
            "Format max_formatted_len body must return an integer literal so the bound can be proven"
                .to_string(),
            span,
        )
    })
}

fn check_format_writer_against_bound(
    body: &[TypedStmt],
    bound: u64,
    span: Span,
) -> Result<(), SemaError> {
    let mut saw = false;
    collect_format_string_returns(body, span, &mut |need| {
        saw = true;
        if need > bound {
            return Err(type_error(
                format!("Format.format exceeds proven max_formatted_len bound ({need} > {bound})"),
                span,
            ));
        }
        Ok(())
    })?;
    if !saw {
        return Err(type_error(
            "Format.format body must return a string expression whose bound can be proven"
                .to_string(),
            span,
        ));
    }
    Ok(())
}

fn collect_format_bound_returns(
    body: &[TypedStmt],
    span: Span,
    on_ret: &mut dyn FnMut(u64) -> Result<(), SemaError>,
) -> Result<(), SemaError> {
    for s in body {
        match &s.kind {
            TypedStmtKind::Return(Some(e)) => match &e.kind {
                TypedExprKind::Int(text) => {
                    let v = parse_int_literal(text).ok_or_else(|| {
                        type_error(
                            "Format max_formatted_len must return an integer literal".to_string(),
                            span,
                        )
                    })?;
                    if v < 0 {
                        return Err(type_error(
                            "Format max_formatted_len must return a non-negative integer"
                                .to_string(),
                            span,
                        ));
                    }
                    on_ret(v as u64)?;
                }
                _ => {
                    return Err(type_error(
                        "Format max_formatted_len body must return an integer literal so the bound can be proven"
                            .to_string(),
                        span,
                    ));
                }
            },
            TypedStmtKind::Return(None) => {
                return Err(type_error(
                    "Format max_formatted_len body must return an integer literal so the bound can be proven"
                        .to_string(),
                    span,
                ));
            }
            TypedStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_format_bound_returns(&arm.body, span, on_ret)?;
                }
            }
            TypedStmtKind::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                collect_format_bound_returns(then_branch, span, on_ret)?;
                for e in elifs {
                    collect_format_bound_returns(&e.body, span, on_ret)?;
                }
                if let Some(eb) = else_branch {
                    collect_format_bound_returns(eb, span, on_ret)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn format_expr_max_len(e: &TypedExpr) -> Result<u64, SemaError> {
    match &e.kind {
        TypedExprKind::Str(text) => Ok(crate::eval::value::decode_str(text).len() as u64),
        TypedExprKind::Binary(BinOp::Add, l, r) => {
            Ok(format_expr_max_len(l)? + format_expr_max_len(r)?)
        }
        TypedExprKind::Call {
            callee: CalleeKey::Method(_, m),
            ..
        } if m == "format" => match &e.ty {
            Type::String(n) => {
                let n = literal_array_len(n).ok_or_else(|| {
                    type_error(
                        "Format.format return capacity must be a literal".to_string(),
                        Span::default(),
                    )
                })?;
                u64::try_from(n).map_err(|_| {
                    type_error(
                        "Format.format return capacity is out of range".to_string(),
                        Span::default(),
                    )
                })
            }
            _ => Err(type_error(
                "Format.format call must return `String[..N]`".to_string(),
                Span::default(),
            )),
        },
        _ => Err(type_error(
            "Format.format body must return a string literal, string `+`, or `.format()` call so the bound can be proven"
                .to_string(),
            Span::default(),
        )),
    }
}

fn collect_format_string_returns(
    body: &[TypedStmt],
    span: Span,
    on_ret: &mut dyn FnMut(u64) -> Result<(), SemaError>,
) -> Result<(), SemaError> {
    for s in body {
        match &s.kind {
            TypedStmtKind::Return(Some(e)) => {
                let need = format_expr_max_len(e).map_err(|err| type_error(err.message, span))?;
                on_ret(need)?;
            }
            TypedStmtKind::Return(None) => {
                return Err(type_error(
                    "Format.format body must return a string expression whose bound can be proven"
                        .to_string(),
                    span,
                ));
            }
            TypedStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_format_string_returns(&arm.body, span, on_ret)?;
                }
            }
            TypedStmtKind::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                collect_format_string_returns(then_branch, span, on_ret)?;
                for e in elifs {
                    collect_format_string_returns(&e.body, span, on_ret)?;
                }
                if let Some(eb) = else_branch {
                    collect_format_string_returns(eb, span, on_ret)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn check_stmts(
    stmts: &[Stmt],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedStmt>, SemaError> {
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        out.push(check_stmt(s, fctx, mctx)?);
    }
    Ok(out)
}

fn check_stmt(stmt: &Stmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    match stmt {
        Stmt::Assign(a) => check_assign(a, fctx, mctx),
        Stmt::If(i) => check_if(i, fctx, mctx),
        Stmt::Match(m) => check_match(m, fctx, mctx),
        Stmt::For(f) => check_for(f, fctx, mctx),
        Stmt::While(w) => check_while(w, fctx, mctx),
        Stmt::Break(span) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::Break,
        }),
        Stmt::Continue(span) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::Continue,
        }),
        Stmt::Pass(span) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::Pass,
        }),
        Stmt::Return(span, e) => check_return(*span, e, fctx, mctx),
        Stmt::Assert(a) => check_assert(a, fctx, mctx),
        Stmt::Defer(d) => check_defer(d, fctx, mctx),
        Stmt::With(w) => check_with(w, fctx, mctx),
        Stmt::Send(span, e) => check_send_stmt(*span, e, fctx, mctx),
        Stmt::Expr(span, e) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::ExprStmt(check_expr(e, None, fctx, mctx)?),
        }),
        Stmt::Dmb(attr) => check_dmb(attr, mctx),
        Stmt::ComptimeIf(c) => Err(unimplemented_at("`comptime if` is", c.span)),
        Stmt::ComptimeAssert(span, cond, message) => {
            check_comptime_assert(*span, cond, message, fctx, mctx)
        }
    }
}

fn check_dmb(attr: &ast::Attr, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let allowed_module = |expected: &[&str]| {
        mctx.loader_key.len() == expected.len()
            && mctx
                .loader_key
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual == *expected)
    };
    if !allowed_module(&crate::loader::RUNTIME_MODULE_KEY)
        && !allowed_module(&["drivers", "display"])
    {
        return Err(SemaError::at(
            "intrinsic",
            "`@dmb` is legal only inside the sealed runtime and display driver modules".to_string(),
            attr.span,
        ));
    }
    if attr.args.len() != 1 {
        return Err(SemaError::at(
            "intrinsic",
            "`@dmb` takes exactly one argument: `ishst` or `ishld`".to_string(),
            attr.span,
        ));
    }
    let arg = &attr.args[0];
    if arg.label.is_some() || arg.mode != AccessMode::Read {
        return Err(SemaError::at(
            "intrinsic",
            "`@dmb` takes a positional barrier option (`ishst` or `ishld`), not a labeled \
             or `mut`/`take` argument"
                .to_string(),
            arg.span,
        ));
    }
    let key = match &arg.value {
        Expr::Name(_, name) if name == "ishst" => "dmb.ishst",
        Expr::Name(_, name) if name == "ishld" => "dmb.ishld",
        _ => {
            return Err(SemaError::at(
                "intrinsic",
                "`@dmb` option must be `ishst` or `ishld`".to_string(),
                arg.span,
            ));
        }
    };
    let kind = match key {
        "dmb.ishst" => TypedExprKind::Intrinsic {
            key: "dmb.ishst".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
        "dmb.ishld" => TypedExprKind::Intrinsic {
            key: "dmb.ishld".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
        _ => unreachable!("option gated above"),
    };
    Ok(TypedStmt {
        span: attr.span,
        kind: TypedStmtKind::ExprStmt(TypedExpr {
            span: attr.span,
            ty: Type::Unit,
            kind,
        }),
    })
}

fn check_comptime_assert(
    span: Span,
    cond: &Expr,
    message: &Option<Expr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(cond, Some(&Type::Bool), fctx, mctx)?;
    let message = match message {
        Some(msg) => match msg {
            Expr::Str(..) => Some(check_expr(msg, None, fctx, mctx)?),
            other => {
                return Err(type_error(
                    "comptime assert message must be a text literal".to_string(),
                    other.span(),
                ));
            }
        },
        None => None,
    };
    Ok(TypedStmt {
        span,
        kind: TypedStmtKind::ComptimeAssert {
            span,
            cond,
            message,
        },
    })
}

fn check_if(i: &IfStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(&i.cond, Some(&Type::Bool), fctx, mctx)?;
    let then_branch = scoped(fctx, |fctx| check_stmts(&i.then_branch, fctx, mctx))?;
    let mut elifs = Vec::with_capacity(i.elifs.len());
    for elif in &i.elifs {
        let ec = check_expr(&elif.cond, Some(&Type::Bool), fctx, mctx)?;
        let eb = scoped(fctx, |fctx| check_stmts(&elif.body, fctx, mctx))?;
        elifs.push(TypedElif { cond: ec, body: eb });
    }
    let else_branch = match &i.else_branch {
        Some(b) => Some(scoped(fctx, |fctx| check_stmts(b, fctx, mctx))?),
        None => None,
    };
    Ok(TypedStmt {
        span: i.span,
        kind: TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        },
    })
}

fn check_while(w: &WhileStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let budget = resolve_loop_budget(w.budget.as_ref(), w.span, fctx, mctx)?;
    let cond = check_expr(&w.cond, Some(&Type::Bool), fctx, mctx)?;
    let body = scoped(fctx, |fctx| check_stmts(&w.body, fctx, mctx))?;
    Ok(TypedStmt {
        span: w.span,
        kind: TypedStmtKind::While { cond, body, budget },
    })
}

fn check_match(m: &MatchStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let discard_ok = match &m.discard {
        Some(attr) => {
            check_discard_attr(attr)?;
            true
        }
        None => false,
    };
    let scrutinee = check_expr(&m.scrutinee, None, fctx, mctx)?;
    let sty = scrutinee.ty.clone();
    let outcome_match = matches!(&sty, Type::Named(n, targs)
        if n == "CompletionOutcome" && targs.is_empty());
    let mut arms = Vec::with_capacity(m.arms.len());
    for arm in &m.arms {
        let unknown_arm = outcome_match && pattern_can_match_unknown(&arm.pattern);
        if unknown_arm {
            fctx.unknown_outcome_arms += 1;
        }
        let checked = scoped(fctx, |fctx| {
            let pattern = check_pattern(&arm.pattern, &sty, fctx, mctx)?;
            let guard = match &arm.guard {
                Some(g) => Some(check_expr(g, Some(&Type::Bool), fctx, mctx)?),
                None => None,
            };
            let body = check_stmts(&arm.body, fctx, mctx)?;
            Ok((pattern, guard, body))
        });
        if unknown_arm {
            fctx.unknown_outcome_arms -= 1;
        }
        let (pattern, guard, body) = checked?;
        arms.push(TypedMatchArm {
            pattern,
            guard,
            body,
        });
    }
    if !discard_ok {
        check_no_silent_err_discard(&sty, &arms, &m.arms, m.span)?;
    }
    Ok(TypedStmt {
        span: m.span,
        kind: TypedStmtKind::Match { scrutinee, arms },
    })
}

fn check_discard_attr(attr: &crate::syntax::ast::Attr) -> Result<(), SemaError> {
    debug_assert_eq!(attr.name, "discard");
    if attr.args.len() != 1 {
        return Err(SemaError::at(
            "sema",
            "`@discard` takes exactly one argument `reason=\"...\"` (02-language.md §13)"
                .to_string(),
            attr.span,
        ));
    }
    let a = &attr.args[0];
    let Some(label) = a.label.as_deref() else {
        return Err(SemaError::at(
            "sema",
            "`@discard` takes `reason=\"...\"` (labeled); a positional argument is not the \
             deliberate-discard spelling (02-language.md §13)"
                .to_string(),
            a.span,
        ));
    };
    if label != "reason" {
        return Err(SemaError::at(
            "sema",
            format!("`@discard` takes `reason=\"...\"`; found `{label}=` (02-language.md §13)"),
            a.span,
        ));
    }
    if a.mode != AccessMode::Read {
        return Err(SemaError::at(
            "sema",
            "`@discard`'s `reason=` is a string literal, not a `mut`/`take` place".to_string(),
            a.span,
        ));
    }
    match &a.value {
        Expr::Str(_, text) if !text.is_empty() => Ok(()),
        Expr::Str(_, _) => Err(SemaError::at(
            "sema",
            "`@discard(reason=\"...\")` requires a non-empty reason string".to_string(),
            a.span,
        )),
        _ => Err(SemaError::at(
            "sema",
            "`@discard(reason=\"...\")` requires a string literal reason".to_string(),
            a.span,
        )),
    }
}

fn result_err_is_call_error(ty: &Type) -> bool {
    match ty {
        Type::Result(_, err) => matches!(&**err, Type::Named(n, _) if n == "CallError"),
        _ => false,
    }
}

fn result_err_is_capacity_error(ty: &Type) -> bool {
    match ty {
        Type::Result(_, err) => {
            matches!(&**err, Type::Named(n, targs) if n == "CapacityError" && targs.is_empty())
        }
        _ => false,
    }
}

fn check_no_silent_err_discard(
    sty: &Type,
    arms: &[TypedMatchArm],
    ast_arms: &[MatchArm],
    match_span: Span,
) -> Result<(), SemaError> {
    let err_name = if result_err_is_call_error(sty) {
        "CallError"
    } else if result_err_is_capacity_error(sty) {
        "CapacityError"
    } else {
        return Ok(());
    };
    for (arm, ast_arm) in arms.iter().zip(ast_arms.iter()) {
        if err_arm_is_silent_discard(&arm.pattern, &arm.body) {
            let mut e = SemaError::at(
                "sema",
                format!(
                    "silent `Err` discard of `{err_name}` — consume the error, or annotate the \
                     `match` with `@discard(reason=\"...\")` (02-language.md §9.4)"
                ),
                ast_arm.span,
            );
            e.extra_lines = vec![
                "  no silent `Err` discard without `@discard(reason=)`".to_string(),
                "  plans/M13.md item L / decision 9".to_string(),
            ];
            let _ = match_span;
            return Err(e);
        }
    }
    Ok(())
}

fn err_arm_is_silent_discard(pattern: &TypedPattern, body: &[TypedStmt]) -> bool {
    match &pattern.kind {
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } if (enum_name == "Result" || enum_name.is_empty()) && variant == "Err" => {
            match payload.first() {
                Some(inner) => pattern_is_silent_discard(inner, body),
                None => true,
            }
        }
        TypedPatternKind::Or(alts) => alts.iter().any(|a| err_arm_is_silent_discard(a, body)),
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {
            pattern_is_silent_discard(pattern, body)
        }
        TypedPatternKind::Take(inner) => err_arm_is_silent_discard(inner, body),
        _ => false,
    }
}

fn pattern_is_silent_discard(pattern: &TypedPattern, body: &[TypedStmt]) -> bool {
    match &pattern.kind {
        TypedPatternKind::Wildcard => true,
        TypedPatternKind::Binding(name) => !typed_stmts_use_local(body, name),
        TypedPatternKind::Take(inner) => pattern_is_silent_discard(inner, body),
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            !items.is_empty() && items.iter().all(|i| pattern_is_silent_discard(i, body))
        }
        TypedPatternKind::Variant { payload, .. } => {
            payload.is_empty() || payload.iter().all(|p| pattern_is_silent_discard(p, body))
        }
        TypedPatternKind::Or(alts) => {
            !alts.is_empty() && alts.iter().all(|a| pattern_is_silent_discard(a, body))
        }
        TypedPatternKind::Literal(_) => false,
    }
}

fn typed_stmts_use_local(stmts: &[TypedStmt], name: &str) -> bool {
    let mut found = false;
    for s in stmts {
        walk_typed_stmt_locals(s, &mut |n| {
            if n == name {
                found = true;
            }
        });
        if found {
            return true;
        }
    }
    false
}

fn walk_typed_stmt_locals(s: &TypedStmt, f: &mut dyn FnMut(&str)) {
    match &s.kind {
        TypedStmtKind::Let { value, .. } => walk_typed_expr_locals(value, f),
        TypedStmtKind::Assign { target, value } => {
            walk_typed_expr_locals(target, f);
            walk_typed_expr_locals(value, f);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            walk_typed_expr_locals(cond, f);
            for s in then_branch {
                walk_typed_stmt_locals(s, f);
            }
            for e in elifs {
                walk_typed_expr_locals(&e.cond, f);
                for s in &e.body {
                    walk_typed_stmt_locals(s, f);
                }
            }
            if let Some(b) = else_branch {
                for s in b {
                    walk_typed_stmt_locals(s, f);
                }
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            walk_typed_expr_locals(scrutinee, f);
            for arm in arms {
                for s in &arm.body {
                    walk_typed_stmt_locals(s, f);
                }
                if let Some(g) = &arm.guard {
                    walk_typed_expr_locals(g, f);
                }
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(start, end, _) => {
                    walk_typed_expr_locals(start, f);
                    walk_typed_expr_locals(end, f);
                }
                TypedForIter::Expr(e) => walk_typed_expr_locals(e, f),
            }
            for s in body {
                walk_typed_stmt_locals(s, f);
            }
        }
        TypedStmtKind::While { cond, body, .. } => {
            walk_typed_expr_locals(cond, f);
            for s in body {
                walk_typed_stmt_locals(s, f);
            }
        }
        TypedStmtKind::Return(Some(e))
        | TypedStmtKind::ExprStmt(e)
        | TypedStmtKind::BareSend { expr: e, .. } => walk_typed_expr_locals(e, f),
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            walk_typed_expr_locals(cond, f);
            if let Some(m) = message {
                walk_typed_expr_locals(m, f);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => walk_typed_expr_locals(e, f),
            TypedDeferBody::Suite(stmts) => {
                for s in stmts {
                    walk_typed_stmt_locals(s, f);
                }
            }
        },
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                walk_typed_expr_locals(c, f);
            }
            if let Some(d) = deadline {
                walk_typed_expr_locals(d, f);
            }
            for s in body {
                walk_typed_stmt_locals(s, f);
            }
        }
        TypedStmtKind::Break
        | TypedStmtKind::Continue
        | TypedStmtKind::Pass
        | TypedStmtKind::Return(None) => {}
    }
}

fn walk_typed_expr_locals(e: &TypedExpr, f: &mut dyn FnMut(&str)) {
    match &e.kind {
        TypedExprKind::Local(name) => f(name),
        TypedExprKind::Field(base, _)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Try(base, _)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Panic(base) => walk_typed_expr_locals(base, f),
        TypedExprKind::Index(base, idx) => {
            walk_typed_expr_locals(base, f);
            walk_typed_expr_locals(idx, f);
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            walk_typed_expr_locals(l, f);
            walk_typed_expr_locals(r, f);
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_typed_expr_locals(r, f);
            }
            for a in args {
                if let Some(t) = &a.value {
                    walk_typed_expr_locals(t, f);
                }
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            walk_typed_expr_locals(callee, f);
            for a in args {
                if let Some(t) = &a.value {
                    walk_typed_expr_locals(t, f);
                }
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_typed_expr_locals(r, f);
            }
            for (_, a) in args {
                walk_typed_expr_locals(a, f);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                walk_typed_expr_locals(v, f);
            }
        }
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                walk_typed_expr_locals(i, f);
            }
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                if let Some(t) = &a.value {
                    walk_typed_expr_locals(t, f);
                }
            }
        }
        TypedExprKind::Is(scrut, _) => walk_typed_expr_locals(scrut, f),
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => walk_typed_expr_locals(e, f),
            TypedClosureBody::Suite(stmts) => {
                for s in stmts {
                    walk_typed_stmt_locals(s, f);
                }
            }
        },
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Const(_)
        | TypedExprKind::Unit
        | TypedExprKind::GroupChild(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::FnRef(_)
        | TypedExprKind::Static(_) => {}
    }
}

fn pattern_can_match_unknown(p: &Pattern) -> bool {
    match p {
        Pattern::Wildcard(_) | Pattern::Binding(_, _) => true,
        Pattern::Take(_, inner) => pattern_can_match_unknown(inner),
        Pattern::Or(_, alts) => alts.iter().any(pattern_can_match_unknown),
        Pattern::Variant { variant, .. } => variant == "Unknown",
        Pattern::Literal(_, _) | Pattern::Tuple(_, _) | Pattern::Array(_, _) => false,
    }
}

fn check_return(
    span: Span,
    e: &Option<Expr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    match e {
        Some(expr) => {
            let ret_ty = fctx.ret_ty.clone();
            let te = check_expr(expr, Some(&ret_ty), fctx, mctx)?;
            Ok(TypedStmt {
                span,
                kind: TypedStmtKind::Return(Some(te)),
            })
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
            Ok(TypedStmt {
                span,
                kind: TypedStmtKind::Return(None),
            })
        }
    }
}

fn check_assert(
    a: &AssertStmt,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(&a.cond, Some(&Type::Bool), fctx, mctx)?;
    let message = match &a.message {
        Some(msg) => match msg {
            Expr::Str(..) => Some(check_expr(msg, None, fctx, mctx)?),
            other => {
                return Err(type_error(
                    "assert message must be a text literal".to_string(),
                    other.span(),
                ));
            }
        },
        None => None,
    };
    Ok(TypedStmt {
        span: a.span,
        kind: TypedStmtKind::Assert { cond, message },
    })
}

fn check_for(f: &ForStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let iterable_taken = matches!(&f.iterable, Expr::Unary(_, UnaryOp::Take, _));
    let raw_iterable: &Expr = match &f.iterable {
        Expr::Unary(_, UnaryOp::Take, inner) => inner.as_ref(),
        other => other,
    };
    let (elem_ty, iter) = match raw_iterable {
        Expr::Range(rspan, from, to, incl) => {
            let (ft, tt) = check_same_type_operands(from, to, fctx, mctx)?;
            if is_untrusted_type(&ft.ty) || is_untrusted_type(&tt.ty) {
                let bad = if is_untrusted_type(&ft.ty) {
                    from.span()
                } else {
                    to.span()
                };
                return Err(untrusted_use_error("a range bound", bad));
            }
            if !is_integer_scalar(&ft.ty) {
                return Err(type_error(
                    format!(
                        "range endpoints must be an integer type, found `{}`",
                        types::render_type(&ft.ty)
                    ),
                    *rspan,
                ));
            }
            let ety = ft.ty.clone();
            (ety, TypedForIter::Range(ft, tt, *incl))
        }
        other => {
            let te = check_expr(other, None, fctx, mctx)?;
            match &te.ty {
                Type::Array(elem, _) => {
                    let ety = (**elem).clone();
                    let te = if iterable_taken {
                        TypedExpr {
                            span: expr_span(&f.iterable),
                            ty: te.ty.clone(),
                            kind: TypedExprKind::Take(Box::new(te)),
                        }
                    } else {
                        te
                    };
                    (ety, TypedForIter::Expr(te))
                }
                _ => {
                    return Err(type_error(
                        format!(
                            "`for` requires a range or fixed array, found `{}`",
                            types::render_type(&te.ty)
                        ),
                        other.span(),
                    ));
                }
            }
        }
    };
    let body = scoped(fctx, |fctx| {
        bind_local(fctx, &f.name, elem_ty.clone(), f.span)?;
        check_stmts(&f.body, fctx, mctx)
    })?;
    let budget = resolve_loop_budget(f.budget.as_ref(), f.span, fctx, mctx)?;
    Ok(TypedStmt {
        span: f.span,
        kind: TypedStmtKind::For {
            name: f.name.clone(),
            elem_ty,
            take_binding: f.take_binding,
            iter,
            body,
            budget,
        },
    })
}

fn resolve_loop_budget(
    budget: Option<&ast::Attr>,
    loop_span: Span,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<Option<u64>, SemaError> {
    match budget {
        None => {
            if fctx.in_async {
                Ok(None)
            } else {
                let mut sites = mctx.unbounded_sync_loops.borrow_mut();
                let ordinal = sites.iter().filter(|s| s.fn_name == fctx.fn_name).count();
                sites.push(crate::sema::typed::UnboundedSyncLoop {
                    fn_name: fctx.fn_name.clone(),
                    span: loop_span,
                    ordinal,
                });
                Ok(None)
            }
        }
        Some(attr) => {
            let n = parse_budget_bound_attr(attr, mctx)?;
            if fctx.in_async { Ok(None) } else { Ok(Some(n)) }
        }
    }
}

fn parse_budget_bound_attr(attr: &ast::Attr, mctx: &ModuleCtx) -> Result<u64, SemaError> {
    if attr.name != "budget" {
        return Err(SemaError::at(
            "sema",
            format!(
                "only `@budget(bound=N)` may annotate a loop; found `@{}`",
                attr.name
            ),
            attr.span,
        ));
    }
    if attr.args.len() != 1 {
        return Err(SemaError::at(
            "sema",
            "`@budget` on a loop takes exactly one argument `bound=N` (02-language.md §8.1)"
                .to_string(),
            attr.span,
        ));
    }
    let arg = &attr.args[0];
    match &arg.label {
        Some(label) if label == "bound" => {}
        Some(other) => {
            return Err(SemaError::at(
                "sema",
                format!(
                    "`@budget` on a loop takes `bound=N`; found `{other}=` (02-language.md §8.1)"
                ),
                arg.span,
            ));
        }
        None => {
            return Err(SemaError::at(
                "sema",
                "`@budget` on a loop takes `bound=N` (labeled); a positional argument is not the sync-loop discharge (02-language.md §8.1)"
                    .to_string(),
                arg.span,
            ));
        }
    }
    if arg.mode != AccessMode::Read {
        return Err(SemaError::at(
            "sema",
            "`@budget(bound=N)`'s `N` is a comptime integer, not a `mut`/`take` place".to_string(),
            arg.span,
        ));
    }
    match &arg.value {
        Expr::Int(span, text) => {
            let n: i128 = text.parse().map_err(|_| {
                SemaError::at(
                    "sema",
                    format!("`@budget(bound=N)` requires an integer literal; found `{text}`"),
                    *span,
                )
            })?;
            budget_bound_from_i128(n, *span)
        }
        Expr::Name(span, name) => budget_bound_from_const_name(name, *span, attr.span, mctx),
        other => Err(SemaError::at(
            "sema",
            "`@budget(bound=N)` requires a comptime-known integer literal or the name of a \
             module-level `const` whose comptime value is one or more for N \
             (02-language.md §8.1, 03-hardware.md §3.1)"
                .to_string(),
            other.span(),
        )),
    }
}

fn budget_bound_from_const_name(
    name: &str,
    name_span: Span,
    attr_span: Span,
    mctx: &ModuleCtx,
) -> Result<u64, SemaError> {
    let Some(ty) = mctx.consts.get(name) else {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, which is not a module-level `const` \
                 visible here; a loop bound is an integer literal or the name of a \
                 module-level `const` whose comptime value is one or more — a name a \
                 `comptime if` removed, a local, or a type is not one (02-language.md §8.1, \
                 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    };
    if !is_integer_scalar(ty) {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, whose type is not an integer; a loop \
                 bound is a count of trips (02-language.md §8.1, 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    }
    let Some(init) = mctx.const_values.get(name) else {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, which is not a module-level `const` \
                 visible here; a loop bound is an integer literal or the name of a \
                 module-level `const` whose comptime value is one or more (02-language.md \
                 §8.1, 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    };
    let n = match init {
        Expr::Int(_, text) => parse_int_literal(text).ok_or_else(|| {
            SemaError::at(
                "sema",
                format!(
                    "`@budget(bound=N)`'s `N` is `{name}`, whose value `{text}` is not an \
                     integer literal (02-language.md §8.1)"
                ),
                attr_span,
            )
        })?,
        Expr::Name(_, other) => {
            return budget_bound_from_const_name(other, name_span, attr_span, mctx);
        }
        _ => {
            return Err(SemaError::at(
                "sema",
                format!(
                    "`@budget(bound=N)`'s `N` is `{name}`, whose initializer is not a \
                     comptime integer literal (02-language.md §8.1, 03-hardware.md §3.1)"
                ),
                attr_span,
            ));
        }
    };
    if n < 1 {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, whose value is {n}; a loop bound is \
                 a comptime-known integer ≥ 1 (02-language.md §8.1, 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    }
    budget_bound_from_i128(n, attr_span)
}

fn budget_bound_from_i128(n: i128, span: Span) -> Result<u64, SemaError> {
    if n < 1 {
        return Err(SemaError::at(
            "sema",
            format!("`@budget(bound=N)` requires N ≥ 1; found {n}"),
            span,
        ));
    }
    u64::try_from(n).map_err(|_| {
        SemaError::at(
            "sema",
            format!("`@budget(bound=N)` value {n} does not fit a trip counter"),
            span,
        )
    })
}

fn check_defer(d: &DeferStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    if let Some((what, span)) = scan_defer_forbidden(&d.body) {
        return Err(type_error(format!("defer body cannot {what}"), span));
    }
    let body = match &d.body {
        DeferBody::Expr(e) => TypedDeferBody::Expr(Box::new(check_expr(e, None, fctx, mctx)?)),
        DeferBody::Suite(stmts) => TypedDeferBody::Suite(check_stmts(stmts, fctx, mctx)?),
    };
    Ok(TypedStmt {
        span: d.span,
        kind: TypedStmtKind::Defer(body),
    })
}

fn check_assign(
    a: &AssignStmt,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    if matches!(a.value, Expr::Closure(_)) {
        return Err(type_error("closures cannot be stored".to_string(), a.span));
    }
    if let Expr::Name(_, name) = &a.target {
        let already_bound = if a.ty.is_some() {
            fctx.lookup_innermost(name)
        } else {
            fctx.lookup_local(name)
        };
        if already_bound.is_some() {
            let target_t = check_expr(&a.target, None, fctx, mctx)?;
            let value_t = if a.op == AssignOp::Assign {
                check_expr(&a.value, Some(&target_t.ty), fctx, mctx)?
            } else {
                check_compound_assign(a.op, &target_t, &a.value, a.span, fctx, mctx)?
            };
            return Ok(TypedStmt {
                span: a.span,
                kind: TypedStmtKind::Assign {
                    target: target_t,
                    value: value_t,
                },
            });
        }
        if a.op != AssignOp::Assign {
            return Err(type_error(
                "compound assignment requires an existing local".to_string(),
                a.span,
            ));
        }
        let (ty, value_t) = match &a.ty {
            Some(ann) => {
                let resolved = mctx.resolve_type(ann, &fctx.local_pools)?;
                let vt = check_expr(&a.value, Some(&resolved), fctx, mctx)?;
                (resolved, vt)
            }
            None => {
                let vt = check_expr(&a.value, None, fctx, mctx)?;
                let t = vt.ty.clone();
                (t, vt)
            }
        };
        bind_local(fctx, name, ty.clone(), a.span)?;
        return Ok(TypedStmt {
            span: a.span,
            kind: TypedStmtKind::Let {
                name: name.clone(),
                ty,
                value: value_t,
            },
        });
    }
    let target_t = check_expr(&a.target, None, fctx, mctx)?;
    let value_t = if a.op == AssignOp::Assign {
        check_expr(&a.value, Some(&target_t.ty), fctx, mctx)?
    } else {
        check_compound_assign(a.op, &target_t, &a.value, a.span, fctx, mctx)?
    };
    Ok(TypedStmt {
        span: a.span,
        kind: TypedStmtKind::Assign {
            target: target_t,
            value: value_t,
        },
    })
}

fn check_compound_assign(
    op: AssignOp,
    target: &TypedExpr,
    value: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
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
    let value_t = check_expr(value, Some(&target.ty), fctx, mctx)?;
    let result = build_binop_expr(binop, target.clone(), value_t, span, mctx)?;
    if !types_eq(&result.ty, &target.ty) {
        return Err(type_error(
            format!(
                "`{}` would change the type of the target from `{}` to `{}`",
                op.as_str(),
                types::render_type(&target.ty),
                types::render_type(&result.ty)
            ),
            span,
        ));
    }
    Ok(result)
}

pub(crate) fn check_pattern(
    p: &Pattern,
    scrutinee: &Type,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedPattern, SemaError> {
    match p {
        Pattern::Wildcard(_) => Ok(TypedPattern {
            span: p.span(),
            ty: scrutinee.clone(),
            kind: TypedPatternKind::Wildcard,
        }),
        Pattern::Literal(span, expr) => {
            let te = check_expr(expr, Some(scrutinee), fctx, mctx)?;
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Literal(Box::new(te)),
            })
        }
        Pattern::Binding(span, name) => {
            bind_local(fctx, name, scrutinee.clone(), *span)?;
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Binding(name.clone()),
            })
        }
        Pattern::Take(span, inner) => {
            let tp = check_pattern(inner, scrutinee, fctx, mctx)?;
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Take(Box::new(tp)),
            })
        }
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
            let mut typed_payload = Vec::with_capacity(payload.len());
            for (sp, ty) in payload.iter().zip(payload_types.iter()) {
                typed_payload.push(check_pattern(sp, ty, fctx, mctx)?);
            }
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Variant {
                    enum_name: resolved_enum_name(scrutinee),
                    variant: variant.clone(),
                    payload: typed_payload,
                },
            })
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
            let mut typed_items = Vec::with_capacity(items.len());
            for (sp, ty) in items.iter().zip(elems.iter()) {
                typed_items.push(check_pattern(sp, ty, fctx, mctx)?);
            }
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Tuple(typed_items),
            })
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
            let mut typed_items = Vec::with_capacity(items.len());
            for sp in items {
                typed_items.push(check_pattern(sp, elem, fctx, mctx)?);
            }
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Array(typed_items),
            })
        }
        Pattern::Or(span, alts) => {
            let mut typed_alts = Vec::with_capacity(alts.len());
            for alt in alts {
                typed_alts.push(check_pattern(alt, scrutinee, fctx, mctx)?);
            }
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Or(typed_alts),
            })
        }
    }
}

fn resolved_enum_name(ty: &Type) -> String {
    match ty {
        Type::Option(_) => "Option".to_string(),
        Type::Result(_, _) => "Result".to_string(),
        Type::Named(name, _) => name.clone(),
        other => unreachable!(
            "resolved_enum_name: `{}` is not an enum-shaped type",
            types::render_type(other)
        ),
    }
}

pub(crate) fn literal_array_len(e: &Expr) -> Option<i128> {
    match e {
        Expr::Int(_, text) => parse_int_literal(text),
        _ => None,
    }
}

pub(crate) const MAX_ARRAY_LEN: i128 = 65_536;

pub(crate) const MAX_STRING_CAPACITY: i128 = MAX_ARRAY_LEN;

pub(crate) fn array_len_fits(n: i128) -> bool {
    (0..=MAX_ARRAY_LEN).contains(&n)
}

pub(crate) fn string_capacity_fits(n: i128) -> bool {
    (0..=MAX_STRING_CAPACITY).contains(&n)
}

fn variant_payload_types_for(
    scrutinee: &Type,
    enum_name: Option<&str>,
    variant: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<Vec<Type>, SemaError> {
    let sum_name = match scrutinee {
        Type::Option(_) => Some("Option"),
        Type::Result(_, _) => Some("Result"),
        Type::Named(name, _) => Some(name.as_str()),
        _ => None,
    };
    if let (Some(expected_name), Some(n)) = (sum_name, enum_name) {
        if n != expected_name {
            let article = match expected_name {
                "Option" | "Result" | "CallError" => "an",
                _ => "a",
            };
            return Err(type_error(
                format!("expected {article} `{expected_name}` pattern, found `{n}`"),
                span,
            ));
        }
    }
    let ctors = match crate::sema::sum::sum_ctors(scrutinee, mctx) {
        Ok(c) => c,
        Err(e) => {
            return Err(SemaError::at(e.category, e.message, span));
        }
    };
    match ctors.into_iter().find(|(name, _)| name == variant) {
        Some((_, payloads)) => Ok(payloads),
        None => {
            let label = match scrutinee {
                Type::Option(_) => "`Option`".to_string(),
                Type::Result(_, _) => "`Result`".to_string(),
                Type::Named(name, _) if name == "CallError" => "`CallError`".to_string(),
                Type::Named(name, _) => format!("enum `{name}`"),
                _ => types::render_type(scrutinee),
            };
            Err(type_error(
                format!("{label} has no variant `{variant}`"),
                span,
            ))
        }
    }
}

pub(crate) fn decl_variant_payload_types(dv: &types::DeclVariant) -> Vec<Type> {
    match &dv.payload {
        DeclVariantPayload::None => vec![],
        DeclVariantPayload::Tuple(types_) => types_.clone(),
        DeclVariantPayload::Named(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
    }
}

pub(crate) fn check_expr(
    expr: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let mut actual = synth_expr(expr, expected, fctx, mctx)?;
    actual.span = expr_span(expr);
    if let Some(exp) = expected {
        if !types_eq(&actual.ty, exp) {
            if let (Type::Result(exp_ok, exp_err), Type::Result(act_ok, act_err)) =
                (exp, &actual.ty)
            {
                if types::is_inferred_error_set(exp_err) && types_eq(exp_ok, act_ok) {
                    fctx.record_inferred_error((**act_err).clone());
                    return Ok(actual);
                }
            }
            if is_queue_permit(exp) && is_reserve_capacity_result(&actual.ty) {
                mctx.reserve_permit_demands
                    .borrow_mut()
                    .push(expr_span(expr));
                actual.ty = exp.clone();
                return Ok(actual);
            }
            if let Some(msg) = untrusted_coercion_message(exp, &actual.ty) {
                return Err(type_error(msg, expr.span()));
            }
            return Err(type_error(
                format!(
                    "expected `{}`, found `{}`",
                    types::render_type(exp),
                    types::render_type(&actual.ty)
                ),
                expr.span(),
            ));
        }
    }
    Ok(actual)
}

fn expr_span(e: &Expr) -> Span {
    e.span()
}

fn is_queue_permit(ty: &Type) -> bool {
    matches!(ty, Type::Named(n, targs) if n == "QueuePermit" && targs.is_empty())
}

fn is_reserve_capacity_result(ty: &Type) -> bool {
    match ty {
        Type::Result(ok, err) => {
            is_queue_permit(ok)
                && matches!(&**err, Type::Named(n, targs) if n == "CapacityError" && targs.is_empty())
        }
        _ => false,
    }
}

fn synth_expr(
    expr: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match expr {
        Expr::Int(span, text) => synth_int_literal(*span, text, expected),
        Expr::Float(span, text) => synth_float_literal(*span, text, expected),
        Expr::Str(span, text) => {
            if let Some(Type::String(n_expr)) = expected {
                let n = literal_array_len(n_expr).ok_or_else(|| {
                    unimplemented_at("a `String[..N]` capacity that is not a literal is", *span)
                })?;
                let n = u64::try_from(n).map_err(|_| {
                    type_error("`String[..N]` capacity is out of range".to_string(), *span)
                })?;
                let bytes = crate::eval::value::decode_str(text);
                if (bytes.len() as u64) > n {
                    return Err(type_error(
                        format!(
                            "text literal of {} bytes exceeds `String[..{n}]` capacity",
                            bytes.len()
                        ),
                        *span,
                    ));
                }
                return Ok(TypedExpr {
                    span: *span,
                    ty: Type::String(n_expr.clone()),
                    kind: TypedExprKind::Str(text.clone()),
                });
            }
            Ok(TypedExpr {
                span: *span,
                ty: Type::Static(Box::new(Type::Str)),
                kind: TypedExprKind::Str(text.clone()),
            })
        }
        Expr::BStr(span, text) => {
            let len = bstr_byte_len(text);
            let ty = Type::Static(Box::new(Type::Bytes(Some(Box::new(Expr::Int(
                *span,
                len.to_string(),
            ))))));
            Ok(TypedExpr {
                span: *span,
                ty,
                kind: TypedExprKind::BStr(text.clone()),
            })
        }
        Expr::Char(span, text) => Ok(TypedExpr {
            span: *span,
            ty: Type::Char,
            kind: TypedExprKind::Char(text.clone()),
        }),
        Expr::FStr(f) => check_fstr(f, fctx, mctx),
        Expr::Bool(span, v) => Ok(TypedExpr {
            span: *span,
            ty: Type::Bool,
            kind: TypedExprKind::Bool(*v),
        }),
        Expr::Unit(span) => Ok(TypedExpr {
            span: *span,
            ty: Type::Unit,
            kind: TypedExprKind::Unit,
        }),
        Expr::Name(span, name) => synth_name(*span, name, expected, fctx, mctx),
        Expr::Field(base, span, name) => check_field_expr(base, *span, name, expected, fctx, mctx),
        Expr::Index(base, span, args) => synth_index(base, *span, args, fctx, mctx),
        Expr::Call(callee, span, args) => check_call(callee, *span, args, expected, fctx, mctx),
        Expr::Unary(span, UnaryOp::Neg, inner) => {
            check_unary_neg(inner, expected, *span, fctx, mctx)
        }
        Expr::Unary(span, UnaryOp::BitNot, inner) => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            if !is_integer_scalar(&it.ty) {
                return Err(type_error(
                    format!(
                        "`~` requires an integer type, found `{}`",
                        types::render_type(&it.ty)
                    ),
                    *span,
                ));
            }
            let ty = it.ty.clone();
            Ok(TypedExpr {
                span: *span,
                ty,
                kind: TypedExprKind::BitNot(Box::new(it)),
            })
        }
        Expr::Unary(span, UnaryOp::Await, inner) => check_await(inner, *span, fctx, mctx),
        Expr::Unary(span, UnaryOp::Take, inner) => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            let ty = it.ty.clone();
            Ok(TypedExpr {
                span: *span,
                ty,
                kind: TypedExprKind::Take(Box::new(it)),
            })
        }
        Expr::Try(span, inner) => check_try(*span, inner, fctx, mctx),
        Expr::Binary(span, op, l, r) => check_binary(*op, l, r, *span, fctx, mctx),
        Expr::Range(span, _from, _to, _incl) => Err(type_error(
            "a range is only a value directly inside `for`".to_string(),
            *span,
        )),
        Expr::Is(span, scrutinee, pattern) => {
            let st = check_expr(scrutinee, None, fctx, mctx)?;
            let sty = st.ty.clone();
            let pt = check_pattern(pattern, &sty, fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::Is(Box::new(st), Box::new(pt)),
            })
        }
        Expr::Not(span, inner) => {
            let it = check_expr(inner, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::Not(Box::new(it)),
            })
        }
        Expr::And(span, l, r) => {
            let lt = check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            let rt = check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::And(Box::new(lt), Box::new(rt)),
            })
        }
        Expr::Or(span, l, r) => {
            let lt = check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            let rt = check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::Or(Box::new(lt), Box::new(rt)),
            })
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
            let typed_args = check_variant_args(&payload_types, args, *span, fctx, mctx)?;
            let enum_name = resolved_enum_name(&exp);
            if contains_opaque_field(&exp, mctx) {
                return Err(type_error(
                    "P004: opaque `Field` may not be stored in an enum value".to_string(),
                    *span,
                ));
            }
            Ok(TypedExpr {
                span: *span,
                ty: exp,
                kind: TypedExprKind::EnumConstruct {
                    enum_name,
                    variant: name.clone(),
                    args: typed_args,
                },
            })
        }
        Expr::Closure(c) => check_closure(c, expected, fctx, mctx),
        Expr::Send(span, inner) => check_send(inner, *span, fctx, mctx),
        Expr::Tuple(span, items) => synth_tuple(*span, items, expected, fctx, mctx),
        Expr::List(span, items) => synth_list(*span, items, expected, fctx, mctx),
        Expr::ArrayRepeat(span, elem, count) => {
            synth_array_repeat(*span, elem, count, expected, fctx, mctx)
        }
    }
}

fn synth_int_literal(
    span: Span,
    text: &str,
    expected: Option<&Type>,
) -> Result<TypedExpr, SemaError> {
    let value = parse_int_literal(text)
        .ok_or_else(|| type_error("invalid integer literal".to_string(), span))?;
    let ty = match expected {
        Some(t) if is_integer_scalar(t) => {
            check_int_range(value, t, span)?;
            t.clone()
        }
        Some(t) => {
            return Err(type_error(
                format!(
                    "expected `{}`, found an integer literal",
                    types::render_type(t)
                ),
                span,
            ));
        }
        None => {
            if value <= i64::MAX as i128 {
                Type::I64
            } else if value <= u64::MAX as i128 {
                Type::U64
            } else {
                return Err(type_error("integer literal out of range".to_string(), span));
            }
        }
    };
    Ok(TypedExpr {
        span: span,
        ty,
        kind: TypedExprKind::Int(text.to_string()),
    })
}

fn synth_float_literal(
    span: Span,
    text: &str,
    expected: Option<&Type>,
) -> Result<TypedExpr, SemaError> {
    let ty = match expected {
        Some(t) if is_float_scalar(t) => t.clone(),
        Some(t) => {
            return Err(type_error(
                format!(
                    "expected `{}`, found a float literal",
                    types::render_type(t)
                ),
                span,
            ));
        }
        None => Type::F64,
    };
    Ok(TypedExpr {
        span: span,
        ty,
        kind: TypedExprKind::Float(text.to_string()),
    })
}

fn synth_name(
    span: Span,
    name: &str,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Some(ty) = fctx.lookup_local(name) {
        return Ok(TypedExpr {
            span: span,
            ty,
            kind: TypedExprKind::Local(name.to_string()),
        });
    }
    if let Some(ty) = mctx.consts.get(name) {
        return Ok(TypedExpr {
            span: span,
            ty: ty.clone(),
            kind: TypedExprKind::Const(name.to_string()),
        });
    }
    if let Some(info) = mctx.statics.get(name) {
        return Ok(TypedExpr {
            span: span,
            ty: info.ty.clone(),
            kind: TypedExprKind::Static(name.to_string()),
        });
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            return Err(unimplemented_at("generic instantiation is", span));
        }
        return Ok(TypedExpr {
            span: span,
            ty: fn_value_type(&f.decl),
            kind: TypedExprKind::FnRef(CalleeKey::Fn(name.to_string())),
        });
    }
    if mctx.structs.contains_key(name) || mctx.enums.contains_key(name) {
        return Err(type_error(format!("`{name}` is a type, not a value"), span));
    }
    match name {
        "None" => match expected {
            Some(t @ Type::Option(_)) => Ok(TypedExpr {
                span: span,
                ty: t.clone(),
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Option".to_string(),
                    variant: "None".to_string(),
                    args: vec![],
                },
            }),
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

pub(crate) fn fn_value_type(d: &types::DeclFn) -> Type {
    let params = d.params.iter().map(|p| (p.mode, p.ty.clone())).collect();
    Type::Fn(params, Box::new(d.ret.clone()))
}

pub(crate) fn unwrap_own(ty: Type) -> Type {
    match ty {
        Type::Own(_, inner) => *inner,
        other => other,
    }
}

fn check_field_expr(
    base: &Expr,
    span: Span,
    name: &str,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                if let Some((_, d)) = s.assoc_fn(name) {
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        span: span,
                        ty: fn_value_type(d),
                        kind: TypedExprKind::FnRef(key),
                    });
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
                if let Some((_, d)) = e.assoc_fn(name) {
                    if !e.generics.is_empty() || !d.generics.is_empty() {
                        return Err(unimplemented_at("generic instantiation is", span));
                    }
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        span: span,
                        ty: fn_value_type(d),
                        kind: TypedExprKind::FnRef(key),
                    });
                }
                if e.has_member_named(name) {
                    return Err(type_error(
                        format!("cannot reference method `{name}` without calling it"),
                        span,
                    ));
                }
                if e.variants.iter().any(|v| v.name == name) {
                    let (targs, decl) =
                        resolve_enum_for_variant_construction(bname, e, expected, span, mctx)?;
                    let dv = decl
                        .variants
                        .iter()
                        .find(|v| v.name == name)
                        .expect("name membership checked above");
                    if matches!(dv.payload, DeclVariantPayload::None) {
                        return Ok(TypedExpr {
                            span: span,
                            ty: Type::Named(bname.clone(), targs),
                            kind: TypedExprKind::EnumConstruct {
                                enum_name: bname.clone(),
                                variant: name.to_string(),
                                args: vec![],
                            },
                        });
                    }
                    return Err(type_error(
                        format!("variant `{name}` requires a payload"),
                        span,
                    ));
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant or associated function `{name}`"),
                    span,
                ));
            }
            if let Some(variants) = crate::sema::stdlib_enums::variant_strs(bname.as_str())? {
                if variants.contains(&name) {
                    return Ok(TypedExpr {
                        span: span,
                        ty: Type::Named(bname.clone(), vec![]),
                        kind: TypedExprKind::EnumConstruct {
                            enum_name: bname.clone(),
                            variant: name.to_string(),
                            args: vec![],
                        },
                    });
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant `{name}`"),
                    span,
                ));
            }
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    if let Type::Named(cap, targs) = &base_ty {
        if cap == "Mmio" {
            return Err(mmio_bare_selection_error(targs, name, span, mctx));
        }
    }
    if let Type::Named(n, targs) = &base_ty {
        if n == "IoCompletion" {
            let fields =
                crate::mwir::io_completion_fields(targs).map_err(|e| type_error(e, span))?;
            let Some((_, field_ty)) = fields.into_iter().find(|(f, _)| *f == name) else {
                return Err(type_error(
                    format!(
                        "`IoCompletion[P]` has fields `payload`, `status`, and `written_len`; \
                         found `{name}`"
                    ),
                    span,
                ));
            };
            return Ok(TypedExpr {
                span: span,
                ty: field_ty,
                kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
            });
        }
    }
    if matches!(&base_ty, Type::String(_)) {
        if name == "len" {
            return Ok(TypedExpr {
                span: span,
                ty: Type::Usize,
                kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
            });
        }
        return Err(type_error(
            format!(
                "`{}` has field `len` only; found `{name}`",
                types::render_type(&base_ty)
            ),
            span,
        ));
    }
    if matches!(&base_ty, Type::Bytes(None)) {
        if name == "len" {
            return Ok(TypedExpr {
                span: span,
                ty: Type::Usize,
                kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
            });
        }
        return Err(type_error(
            format!("type `Bytes` has no field `{name}`"),
            span,
        ));
    }
    match &base_ty {
        Type::Named(sname, targs) => {
            let s = if targs.is_empty() {
                match mctx.structs.get(sname.as_str()) {
                    Some(s) => std::borrow::Cow::Borrowed(s),
                    None => {
                        return Err(type_error(
                            format!("type `{sname}` has no field `{name}`"),
                            span,
                        ));
                    }
                }
            } else {
                std::borrow::Cow::Owned(generics::instantiate_struct(mctx, sname, targs, span)?)
            };
            if let Some(ty) = s.field_ty(name) {
                check_field_privacy(sname, name, &s, span, mctx)?;
                return Ok(TypedExpr {
                    span: span,
                    ty,
                    kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
                });
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

pub(crate) fn check_field_privacy(
    type_name: &str,
    field: &str,
    s: &StructInfo,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let Some(is_pub) = s.field_is_pub(field) else {
        return Ok(());
    };
    if is_pub {
        return Ok(());
    }
    let decl_mod = mctx
        .struct_decl_module
        .get(type_name)
        .cloned()
        .unwrap_or_else(|| mctx.module_path.clone());
    let decl_parts: Vec<&str> = decl_mod.split('.').collect();
    if decl_parts.as_slice() == crate::loader::IMAGE_RUNTIME_MODULE_KEY {
        return Ok(());
    }
    let use_mod = mctx
        .visibility_home
        .borrow()
        .clone()
        .unwrap_or_else(|| mctx.module_path.clone());
    if use_mod == decl_mod {
        return Ok(());
    }
    if matches!(use_mod.as_str(), "render" | "core.render")
        && mctx
            .struct_decl_module
            .get("Renderer")
            .is_some_and(|module| matches!(module.as_str(), "render" | "core.render"))
        && mctx
            .type_decl_name
            .get("Renderer")
            .is_some_and(|name| name == "Renderer")
    {
        // Canonical Renderer[P] is trusted compiler/runtime glue. Its concrete
        // instantiation receives generated P5 range checks for P's declared
        // fields. Keep user privacy intact everywhere else while allowing that
        // one canonical generic body to inspect the owned frame it validates.
        return Ok(());
    }
    Err(SemaError::at(
        "sema",
        format!(
            "field `{field}` of `{type_name}` is private to module `{decl_mod}`; \
             only that module may construct, read, write, or pattern-bind it \
             (02-language.md §2)"
        ),
        span,
    ))
}

fn synth_index(
    base: &Expr,
    span: Span,
    args: &[Expr],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, n) = base {
        if mctx.structs.contains_key(n) || mctx.enums.contains_key(n) || mctx.fns.contains_key(n) {
            return Err(unimplemented_at("generic instantiation is", span));
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    if args.len() != 1 {
        return Err(type_error(
            format!("indexing takes exactly one argument, found {}", args.len()),
            span,
        ));
    }
    match &base_ty {
        Type::Array(elem, _) => {
            let idx_t = if is_bare_numeric_literal(&args[0]) {
                check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?
            } else {
                let idx_t = check_expr(&args[0], None, fctx, mctx)?;
                if is_untrusted_type(&idx_t.ty) {
                    return Err(untrusted_use_error("an array index", args[0].span()));
                }
                if !types_eq(&idx_t.ty, &Type::Usize) {
                    return Err(type_error(
                        format!(
                            "expected `usize`, found `{}`",
                            types::render_type(&idx_t.ty)
                        ),
                        args[0].span(),
                    ));
                }
                idx_t
            };
            Ok(TypedExpr {
                span: span,
                ty: (**elem).clone(),
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
        }
        Type::Bytes(_) => {
            let idx_t = if is_bare_numeric_literal(&args[0]) {
                check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?
            } else {
                let idx_t = check_expr(&args[0], None, fctx, mctx)?;
                if is_untrusted_type(&idx_t.ty) {
                    return Err(untrusted_use_error("an array index", args[0].span()));
                }
                if !types_eq(&idx_t.ty, &Type::Usize) {
                    return Err(type_error(
                        format!(
                            "expected `usize`, found `{}`",
                            types::render_type(&idx_t.ty)
                        ),
                        args[0].span(),
                    ));
                }
                idx_t
            };
            Ok(TypedExpr {
                span: span,
                ty: Type::U8,
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
        }
        Type::String(_) => {
            let idx_t = if is_bare_numeric_literal(&args[0]) {
                check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?
            } else {
                let idx_t = check_expr(&args[0], None, fctx, mctx)?;
                if is_untrusted_type(&idx_t.ty) {
                    return Err(untrusted_use_error("an array index", args[0].span()));
                }
                if !types_eq(&idx_t.ty, &Type::Usize) {
                    return Err(type_error(
                        format!(
                            "expected `usize`, found `{}`",
                            types::render_type(&idx_t.ty)
                        ),
                        args[0].span(),
                    ));
                }
                idx_t
            };
            Ok(TypedExpr {
                span: span,
                ty: Type::U8,
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
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
) -> Result<TypedExpr, SemaError> {
    if let Some(Type::Tuple(exp_elems)) = expected {
        if exp_elems.iter().any(|ty| contains_opaque_field(ty, mctx)) && mctx.module_path != "field"
        {
            return Err(type_error(
                "P004: opaque `Field` values may not be stored in tuples".to_string(),
                span,
            ));
        }
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
        let mut typed_items = Vec::with_capacity(items.len());
        for (item, ety) in items.iter().zip(exp_elems.iter()) {
            typed_items.push(check_expr(item, Some(ety), fctx, mctx)?);
        }
        return Ok(TypedExpr {
            span: span,
            ty: Type::Tuple(exp_elems),
            kind: TypedExprKind::Tuple(typed_items),
        });
    }
    let mut typed_items = Vec::with_capacity(items.len());
    for item in items {
        typed_items.push(check_expr(item, None, fctx, mctx)?);
    }
    let elems = typed_items.iter().map(|t| t.ty.clone()).collect();
    if typed_items
        .iter()
        .any(|item| contains_opaque_field(&item.ty, mctx))
        && mctx.module_path != "field"
    {
        return Err(type_error(
            "P004: opaque `Field` values may not be stored in tuples".to_string(),
            span,
        ));
    }
    Ok(TypedExpr {
        span: span,
        ty: Type::Tuple(elems),
        kind: TypedExprKind::Tuple(typed_items),
    })
}

fn synth_list(
    span: Span,
    items: &[Expr],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Some(Type::Array(elem, len_expr)) = expected {
        let elem = (**elem).clone();
        if contains_opaque_field(&elem, mctx) && mctx.module_path != "field" {
            return Err(type_error(
                "P004: opaque `Field` values may not be stored in arrays".to_string(),
                span,
            ));
        }
        let len_expr = len_expr.clone();
        if let Some(n) = literal_array_len(&len_expr) {
            if n != items.len() as i128 {
                return Err(type_error(
                    format!("array expects {n} element(s), found {}", items.len()),
                    span,
                ));
            }
        }
        let mut typed_items = Vec::with_capacity(items.len());
        for item in items {
            typed_items.push(check_expr(item, Some(&elem), fctx, mctx)?);
        }
        return Ok(TypedExpr {
            span: span,
            ty: Type::Array(Box::new(elem), len_expr),
            kind: TypedExprKind::List(typed_items),
        });
    }
    if items.is_empty() {
        return Err(type_error(
            "cannot infer the element type of an empty array literal".to_string(),
            span,
        ));
    }
    let first = check_expr(&items[0], None, fctx, mctx)?;
    let elem_ty = first.ty.clone();
    if contains_opaque_field(&elem_ty, mctx) && mctx.module_path != "field" {
        return Err(type_error(
            "P004: opaque `Field` values may not be stored in arrays".to_string(),
            span,
        ));
    }
    let mut typed_items = Vec::with_capacity(items.len());
    typed_items.push(first);
    for item in &items[1..] {
        typed_items.push(check_expr(item, Some(&elem_ty), fctx, mctx)?);
    }
    let len = Expr::Int(span, items.len().to_string());
    Ok(TypedExpr {
        span: span,
        ty: Type::Array(Box::new(elem_ty), Box::new(len)),
        kind: TypedExprKind::List(typed_items),
    })
}

fn synth_array_repeat(
    span: Span,
    elem: &Expr,
    count: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let n = literal_array_len(count).ok_or_else(|| {
        type_error(
            "`[elem; N]` needs a literal usize count (after const-generic substitution)"
                .to_string(),
            count.span(),
        )
    })?;
    if n < 0 {
        return Err(type_error(
            "`[elem; N]` count must be nonnegative".to_string(),
            count.span(),
        ));
    }
    if !array_len_fits(n) {
        return Err(type_error(
            format!("`[elem; N]` count {n} exceeds the {MAX_ARRAY_LEN}-element build limit"),
            count.span(),
        ));
    }
    let n_usize = n as usize;
    let elem_expected = match expected {
        Some(Type::Array(elem_ty, len_expr)) => {
            if let Some(en) = literal_array_len(len_expr) {
                if en != n {
                    return Err(type_error(
                        format!("array expects {en} element(s), found {n}"),
                        span,
                    ));
                }
            }
            Some(elem_ty.as_ref())
        }
        _ => None,
    };
    let first = check_expr(elem, elem_expected, fctx, mctx)?;
    let elem_ty = first.ty.clone();
    if contains_opaque_field(&elem_ty, mctx) && mctx.module_path != "field" {
        return Err(type_error(
            "P004: opaque `Field` values may not be stored in arrays".to_string(),
            span,
        ));
    }
    let mut typed_items = Vec::with_capacity(n_usize);
    typed_items.push(first);
    for _ in 1..n_usize {
        typed_items.push(check_expr(elem, Some(&elem_ty), fctx, mctx)?);
    }
    let len = count.clone();
    Ok(TypedExpr {
        span: span,
        ty: Type::Array(Box::new(elem_ty), Box::new(len)),
        kind: TypedExprKind::List(typed_items),
    })
}

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

pub(crate) fn types_eq(a: &Type, b: &Type) -> bool {
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
        (Type::String(l1), Type::String(l2)) => same_len_expr(l1, l2),
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
        (types::TypeArg::Const(x), types::TypeArg::Bound(y))
        | (types::TypeArg::Bound(x), types::TypeArg::Const(y)) => same_len_expr(x, y),
        (types::TypeArg::Pool(x), types::TypeArg::Pool(y)) => x == y,
        _ => false,
    }
}

fn same_len_expr(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Int(_, t1), Expr::Int(_, t2)) => parse_int_literal(t1) == parse_int_literal(t2),
        (Expr::Name(_, n1), Expr::Name(_, n2)) => n1 == n2,
        (Expr::Bool(_, b1), Expr::Bool(_, b2)) => b1 == b2,
        _ => false,
    }
}

fn check_int_range(value: i128, ty: &Type, span: Span) -> Result<(), SemaError> {
    let (min, max) =
        crate::eval::value::int_bounds(ty).expect("check_int_range called with a non-integer type");
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

pub(crate) use crate::eval::value::parse_int_literal;

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
) -> Result<TypedExpr, SemaError> {
    match inner {
        Expr::Int(ispan, text) => {
            let raw = parse_int_literal(text)
                .ok_or_else(|| type_error("invalid integer literal".to_string(), *ispan))?;
            let value = -raw;
            let ty = match expected {
                Some(t) if is_integer_scalar(t) => {
                    check_int_range(value, t, *ispan)?;
                    t.clone()
                }
                Some(t) => {
                    return Err(type_error(
                        format!(
                            "expected `{}`, found an integer literal",
                            types::render_type(t)
                        ),
                        *ispan,
                    ));
                }
                None => {
                    check_int_range(value, &Type::I64, *ispan)?;
                    Type::I64
                }
            };
            let literal = TypedExpr {
                span: span,
                ty: ty.clone(),
                kind: TypedExprKind::Int(text.clone()),
            };
            Ok(TypedExpr {
                span: span,
                ty,
                kind: TypedExprKind::Neg(Box::new(literal)),
            })
        }
        Expr::Float(_, text) => {
            let te = synth_float_literal(inner.span(), text, expected)?;
            let ty = te.ty.clone();
            Ok(TypedExpr {
                span: span,
                ty,
                kind: TypedExprKind::Neg(Box::new(te)),
            })
        }
        _ => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            if (is_integer_scalar(&it.ty) && is_signed_scalar(&it.ty)) || is_float_scalar(&it.ty) {
                let ty = it.ty.clone();
                Ok(TypedExpr {
                    span: span,
                    ty,
                    kind: TypedExprKind::Neg(Box::new(it)),
                })
            } else {
                Err(type_error(
                    format!(
                        "unary `-` requires a signed integer or float type, found `{}`",
                        types::render_type(&it.ty)
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
) -> Result<TypedExpr, SemaError> {
    if op == BinOp::Add {
        if let Some(out) = check_string_add(l, r, span, fctx, mctx)? {
            return Ok(out);
        }
    }
    let (lt, rt) = check_same_type_operands(l, r, fctx, mctx)?;
    build_binop_expr(op, lt, rt, span, mctx)
}

fn check_string_add(
    l: &Expr,
    r: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Option<TypedExpr>, SemaError> {
    let lt = check_expr(l, None, fctx, mctx)?;
    let lt = coerce_text_literal_to_string(lt, l.span())?;
    let Type::String(ln) = &lt.ty else {
        return Ok(None);
    };
    let ln = literal_array_len(ln).ok_or_else(|| {
        unimplemented_at("a `String[..N]` capacity that is not a literal is", span)
    })?;
    let rt = check_expr(r, None, fctx, mctx)?;
    let rt = coerce_text_literal_to_string(rt, r.span())?;
    let Type::String(rn) = &rt.ty else {
        return Err(type_error(
            format!(
                "expected `String[..N]`, found `{}`",
                types::render_type(&rt.ty)
            ),
            r.span(),
        ));
    };
    let rn = literal_array_len(rn).ok_or_else(|| {
        unimplemented_at("a `String[..N]` capacity that is not a literal is", span)
    })?;
    let sum = ln
        .checked_add(rn)
        .ok_or_else(|| type_error("String concatenation capacity overflows".to_string(), span))?;
    if !string_capacity_fits(sum) {
        return Err(type_error(
            "String concatenation capacity overflows".to_string(),
            span,
        ));
    }
    Ok(Some(TypedExpr {
        span: span,
        ty: Type::String(Box::new(Expr::Int(span, sum.to_string()))),
        kind: TypedExprKind::Binary(BinOp::Add, Box::new(lt), Box::new(rt)),
    }))
}

fn coerce_text_literal_to_string(te: TypedExpr, span: Span) -> Result<TypedExpr, SemaError> {
    match &te.ty {
        Type::String(_) => Ok(te),
        Type::Static(inner) if matches!(inner.as_ref(), Type::Str) => {
            let TypedExprKind::Str(text) = &te.kind else {
                return Err(type_error(
                    "only a text literal coerces from `Static[Str]` to `String[..N]` here"
                        .to_string(),
                    span,
                ));
            };
            let n = crate::eval::value::decode_str(text).len();
            Ok(TypedExpr {
                span: span,
                ty: Type::String(Box::new(Expr::Int(span, n.to_string()))),
                kind: te.kind,
            })
        }
        _ => Ok(te),
    }
}

fn check_fstr(
    f: &crate::syntax::ast::FStringLit,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let desugared = crate::sema::fstring::desugar_fstring(f)?;
    let te = match check_expr(&desugared, None, fctx, mctx) {
        Ok(te) => te,
        Err(e) => return Err(rewrite_fstring_format_error(e)),
    };
    match &te.ty {
        Type::String(_) => Ok(te),
        Type::Static(inner) if matches!(inner.as_ref(), Type::Str) => {
            coerce_text_literal_to_string(te, f.span)
        }
        other => Err(type_error(
            format!(
                "f-string must produce `String[..N]`, found `{}`",
                types::render_type(other)
            ),
            f.span,
        )),
    }
}

fn rewrite_fstring_format_error(e: SemaError) -> SemaError {
    if let Some((ty, method)) = &e.missing_method {
        if method == "format" {
            if ty == "Secret" {
                return types::secret_has_no_format(Span {
                    line: e.line,
                    col: e.col,
                    ..Default::default()
                });
            }
            return SemaError::at(
                "type",
                format!(
                    "f-string operand of type `{ty}` has no Format \
                     (unbounded / no max_formatted_len; 05-library.md §6)"
                ),
                Span {
                    line: e.line,
                    col: e.col,
                    ..Default::default()
                },
            );
        }
    }
    e
}

fn check_same_type_operands(
    a: &Expr,
    b: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(TypedExpr, TypedExpr), SemaError> {
    if is_bare_numeric_literal(a) && !is_bare_numeric_literal(b) {
        let bt = check_expr(b, None, fctx, mctx)?;
        if is_untrusted_type(&bt.ty) {
            let at = check_expr(a, None, fctx, mctx)?;
            return Ok((at, bt));
        }
        let at = check_expr(a, Some(&bt.ty), fctx, mctx)?;
        Ok((at, bt))
    } else {
        let at = check_expr(a, None, fctx, mctx)?;
        if is_untrusted_type(&at.ty) {
            let bt = check_expr(b, None, fctx, mctx)?;
            return Ok((at, bt));
        }
        let bt = check_expr(b, Some(&at.ty), fctx, mctx)?;
        Ok((at, bt))
    }
}

pub(crate) fn is_bare_numeric_literal(e: &Expr) -> bool {
    matches!(e, Expr::Int(..) | Expr::Float(..))
}

fn build_binop_expr(
    op: BinOp,
    l: TypedExpr,
    r: TypedExpr,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    use BinOp::*;
    if is_untrusted_type(&l.ty) || is_untrusted_type(&r.ty) {
        let use_kind = match op {
            Eq | Ne | Lt | Le | Gt | Ge => "a comparison",
            _ => "an arithmetic operand",
        };
        return Err(untrusted_use_error(use_kind, span));
    }
    let ty = l.ty.clone();
    match op {
        Add | Sub | Mul | Div | Rem => {
            if is_numeric_scalar(&ty) {
                return Ok(TypedExpr {
                    span: span,
                    ty,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                });
            }
            if let Type::Named(name, targs) = &ty {
                let method = match op {
                    Add => "add",
                    Sub => "subtract",
                    Mul => "multiply",
                    Div => "divide",
                    Rem => "remainder",
                    _ => unreachable!(),
                };
                let (ret_ty, key) = resolve_operator_method(name, targs, method, &ty, mctx, span)?;
                return Ok(TypedExpr {
                    span: span,
                    ty: ret_ty,
                    kind: TypedExprKind::OpCall(key, Box::new(l), Box::new(r)),
                });
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
                Ok(TypedExpr {
                    span: span,
                    ty,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                })
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
                Ok(TypedExpr {
                    span: span,
                    ty,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                })
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
                return Ok(TypedExpr {
                    span: span,
                    ty: Type::Bool,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                });
            }
            if let Type::Named(name, targs) = &ty {
                if op == Lt {
                    let (ret, key) =
                        resolve_operator_method(name, targs, "less_than", &ty, mctx, span)?;
                    if ret != Type::Bool {
                        return Err(type_error(
                            format!("`{name}.less_than` must return `bool`"),
                            span,
                        ));
                    }
                    return Ok(TypedExpr {
                        span: span,
                        ty: Type::Bool,
                        kind: TypedExprKind::OpCall(key, Box::new(l), Box::new(r)),
                    });
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
            Ok(TypedExpr {
                span: span,
                ty: Type::Bool,
                kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
            })
        }
    }
}

pub(crate) fn is_resource_type(ty: &Type, mctx: &ModuleCtx) -> bool {
    types::resource_propagates(ty, &mut |name, _args| {
        if crate::sema::classes::name_holds_authority(name) {
            return true;
        }
        mctx.structs
            .get(name)
            .map(|s| s.decl.classification == Classification::Resource)
            .or_else(|| {
                mctx.enums
                    .get(name)
                    .map(|e| e.classification == Classification::Resource)
            })
            .unwrap_or(false)
    })
}

fn resolve_operator_method(
    name: &str,
    targs: &[TypeArg],
    method: &str,
    self_ty: &Type,
    mctx: &ModuleCtx,
    span: Span,
) -> Result<(Type, CalleeKey), SemaError> {
    let s = if targs.is_empty() {
        match mctx.structs.get(name) {
            Some(s) => std::borrow::Cow::Borrowed(s),
            None => {
                return Err(missing_method_error(
                    format!("type `{name}` has no operator method `{method}`"),
                    name,
                    method,
                    span,
                ));
            }
        }
    } else {
        std::borrow::Cow::Owned(generics::instantiate_struct(mctx, name, targs, span)?)
    };
    let Some((_, d)) = s.method(method) else {
        return Err(missing_method_error(
            format!("type `{name}` has no operator method `{method}`"),
            name,
            method,
            span,
        ));
    };
    let receiver_read = d
        .receiver
        .as_ref()
        .map(|r| matches!(r.mode, None | Some(AccessMode::Read)))
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
    let key = if targs.is_empty() {
        CalleeKey::Method(name.to_string(), method.to_string())
    } else {
        CalleeKey::MethodInstance(
            generics::canonical_key(InstKind::Struct, name, targs),
            method.to_string(),
        )
    };
    Ok((d.ret.clone(), key))
}

fn check_try(
    span: Span,
    inner: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let inner_t = check_expr(inner, None, fctx, mctx)?;
    match inner_t.ty.clone() {
        Type::Result(t_ok, t_err) => match fctx.ret_ty.clone() {
            Type::Result(_, ret_err) if types::is_inferred_error_set(&ret_err) => {
                if types::is_inferred_error_set(&t_err) {
                    return Err(type_error(
                        "`?` on a function whose error set is not yet inferred — declare \
                         that function above this one (02-language.md §5)"
                            .to_string(),
                        span,
                    ));
                }
                fctx.record_inferred_error(*t_err);
                Ok(TypedExpr {
                    span: span,
                    ty: *t_ok,
                    kind: TypedExprKind::Try(Box::new(inner_t), None),
                })
            }
            Type::Result(_, ret_err) => {
                if types_eq(&t_err, &ret_err) || call_error_e_compatible(&t_err, &ret_err) {
                    Ok(TypedExpr {
                        span: span,
                        ty: *t_ok,
                        kind: TypedExprKind::Try(Box::new(inner_t), None),
                    })
                } else if let Some((conv_ret, key)) = try_from_conversion(&t_err, &ret_err, mctx) {
                    if types_eq(&conv_ret, &ret_err) {
                        Ok(TypedExpr {
                            span: span,
                            ty: *t_ok,
                            kind: TypedExprKind::Try(Box::new(inner_t), Some(key)),
                        })
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
            Type::Option(_) => Ok(TypedExpr {
                span: span,
                ty: *t_inner,
                kind: TypedExprKind::Try(Box::new(inner_t), None),
            }),
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

fn try_from_conversion(
    err_ty: &Type,
    target_ty: &Type,
    mctx: &ModuleCtx,
) -> Option<(Type, CalleeKey)> {
    let Type::Named(name, targs) = target_ty else {
        return None;
    };
    if !targs.is_empty() {
        return None;
    }
    if let Some(s) = mctx.structs.get(name) {
        if let Some((_, d)) = s.assoc_fn("from") {
            let shape_ok = d.generics.is_empty()
                && d.params.len() == 1
                && d.params[0].mode == AccessMode::Take
                && (types_eq(&d.params[0].ty, err_ty)
                    || call_error_e_compatible(&d.params[0].ty, err_ty));
            if shape_ok {
                return Some((
                    d.ret.clone(),
                    CalleeKey::Method(name.clone(), "from".to_string()),
                ));
            }
        }
    }
    if let Some(e) = mctx.enums.get(name) {
        if let Some((_, d)) = e.assoc_fn("from") {
            let shape_ok = d.generics.is_empty()
                && d.params.len() == 1
                && d.params[0].mode == AccessMode::Take
                && (types_eq(&d.params[0].ty, err_ty)
                    || call_error_e_compatible(&d.params[0].ty, err_ty));
            if shape_ok {
                return Some((
                    d.ret.clone(),
                    CalleeKey::Method(name.clone(), "from".to_string()),
                ));
            }
        }
    }
    None
}

fn check_closure(
    c: &ClosureExpr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some(Type::Fn(exp_params, exp_ret)) = expected.cloned() else {
        return Err(type_error(
            "a closure needs a known function type from its call-site context".to_string(),
            c.span,
        ));
    };
    if c.params.len() != exp_params.len() {
        return Err(arity_error(exp_params.len(), c.params.len(), c.span));
    }
    if let Some(span) = scan_closure_await(&c.body) {
        return Err(type_error(
            "a closure cannot contain `await`".to_string(),
            span,
        ));
    }
    fctx.push_scope();
    let saved_async = fctx.in_async;
    fctx.in_async = false;
    let result = check_closure_body(c, &exp_params, &exp_ret, fctx, mctx);
    fctx.in_async = saved_async;
    fctx.pop_scope();
    let (params, body) = result?;
    Ok(TypedExpr {
        span: c.span,
        ty: Type::Fn(exp_params, exp_ret),
        kind: TypedExprKind::Closure { params, body },
    })
}

fn check_closure_body(
    c: &ClosureExpr,
    exp_params: &[(AccessMode, Type)],
    exp_ret: &Type,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(Vec<TypedClosureParam>, TypedClosureBody), SemaError> {
    let mut typed_params = Vec::with_capacity(c.params.len());
    for (cp, (mode, ety)) in c.params.iter().zip(exp_params.iter()) {
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
        fctx.insert_local(cp.name.clone(), pty.clone());
        typed_params.push(TypedClosureParam {
            mode: *mode,
            name: cp.name.clone(),
            ty: pty,
        });
    }
    let body = match &c.body {
        ClosureBody::Expr(e) => {
            let te = check_expr(e, Some(exp_ret), fctx, mctx)?;
            TypedClosureBody::Expr(Box::new(te))
        }
        ClosureBody::Suite(stmts) => {
            let saved_ret = std::mem::replace(&mut fctx.ret_ty, exp_ret.clone());
            let r = check_stmts(stmts, fctx, mctx);
            fctx.ret_ty = saved_ret;
            TypedClosureBody::Suite(r?)
        }
    };
    Ok((typed_params, body))
}

fn call_fn_value(
    callee: TypedExpr,
    args: &[Arg],
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match callee.ty.clone() {
        Type::Fn(params, ret) => {
            let typed_args = check_positional_args(&params, args, span, fctx, mctx)?;
            Ok(TypedExpr {
                span: span,
                ty: *ret,
                kind: TypedExprKind::CallValue(Box::new(callee), typed_args),
            })
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
) -> Result<TypedExpr, SemaError> {
    match callee {
        Expr::Index(inner, ispan, targs) => {
            check_call_index(inner, *ispan, targs, args, span, fctx, mctx)
        }
        Expr::Name(_, name) => check_call_by_name(name, span, args, expected, fctx, mctx),
        Expr::Field(base, fspan, name) => {
            check_call_by_field(base, *fspan, name, span, args, expected, fctx, mctx)
        }
        other => {
            let callee_t = check_expr(other, None, fctx, mctx)?;
            call_fn_value(callee_t, args, span, fctx, mctx)
        }
    }
}

fn check_call_index(
    inner: &Expr,
    ispan: Span,
    targs: &[Expr],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, name) = inner {
        capability_forgery_check(name, "constructed", call_span)?;
        if name == "entropy" {
            return check_entropy_call(targs, args, ispan, call_span);
        }
    }
    if let Expr::Field(base, fspan, mname) = inner {
        if mname == "device" || mname == "pool" || mname == "dma_pool" || mname == "renderer" {
            let base_t = check_expr(base, None, fctx, mctx)?;
            if base_t.ty == image_type() {
                return check_image_bracket_intrinsic(mname, targs, args, *fspan, fctx, mctx);
            }
        }
        if mname == "to" || mname == "checked_to" || mname == "truncate_to" {
            if targs.len() != 1 {
                return Err(type_error(
                    "a conversion needs exactly one type argument".to_string(),
                    ispan,
                ));
            }
            if let Some(name) = capability_name_in_type_expr(&targs[0]) {
                capability_forgery_check(name, "cast to", ispan)?;
            }
            let base_t = check_expr(base, None, fctx, mctx)?;
            if !is_scalar(&base_t.ty) {
                return Err(type_error(
                    format!(
                        "`.{mname}` is only defined for scalar types, found `{}`",
                        types::render_type(&base_t.ty)
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
            return Ok(TypedExpr {
                span: call_span,
                ty: target,
                kind: TypedExprKind::ToScalar(Box::new(base_t)),
            });
        }
        if let Expr::Name(_, bname) = base.as_ref() {
            if fctx.lookup_local(bname).is_none() {
                if let Some(s) = mctx.structs.get(bname.as_str()) {
                    if s.decl.generics.is_empty() {
                        if let Some((_, d)) = s.assoc_fn(mname) {
                            if !d.generics.is_empty() {
                                let recv_ty = Type::Named(bname.clone(), vec![]);
                                return check_method_generic_call(
                                    &recv_ty,
                                    mname,
                                    d,
                                    args,
                                    Some(targs),
                                    call_span,
                                    None,
                                    fctx,
                                    mctx,
                                );
                            }
                        }
                    }
                }
                if let Some(e) = mctx.enums.get(bname.as_str()) {
                    if e.generics.is_empty() {
                        if let Some((_, d)) = e.assoc_fn(mname) {
                            if !d.generics.is_empty() {
                                let recv_ty = Type::Named(bname.clone(), vec![]);
                                return check_method_generic_call(
                                    &recv_ty,
                                    mname,
                                    d,
                                    args,
                                    Some(targs),
                                    call_span,
                                    None,
                                    fctx,
                                    mctx,
                                );
                            }
                        }
                    }
                }
            }
        }
        let base_t = check_expr(base, None, fctx, mctx)?;
        let base_ty = unwrap_own(base_t.ty.clone());
        if let Type::Named(sname, recv_targs) = &base_ty {
            if let Some(s) = if recv_targs.is_empty() {
                mctx.structs
                    .get(sname.as_str())
                    .map(std::borrow::Cow::Borrowed)
            } else if mctx.structs.contains_key(sname.as_str()) {
                Some(std::borrow::Cow::Owned(generics::instantiate_struct(
                    mctx, sname, recv_targs, call_span,
                )?))
            } else {
                None
            } {
                if let Some((_, d)) = s.method(mname) {
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(sname.clone(), recv_targs.clone());
                        return check_method_generic_call(
                            &recv_ty,
                            mname,
                            d,
                            args,
                            Some(targs),
                            call_span,
                            Some(base_t),
                            fctx,
                            mctx,
                        );
                    }
                }
            }
            if recv_targs.is_empty() {
                if let Some(e) = mctx.enums.get(sname.as_str()) {
                    if let Some((_, d)) = e.method(mname) {
                        if !d.generics.is_empty() {
                            let recv_ty = Type::Named(sname.clone(), vec![]);
                            return check_method_generic_call(
                                &recv_ty,
                                mname,
                                d,
                                args,
                                Some(targs),
                                call_span,
                                Some(base_t),
                                fctx,
                                mctx,
                            );
                        }
                    }
                }
            }
        }
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    if let Expr::Name(_, name) = inner {
        if fctx.lookup_local(name).is_none() {
            if let Some(fi) = mctx.fns.get(name) {
                if !fi.decl.generics.is_empty() {
                    let type_args = generics::resolve_call_targs(targs, mctx)?;
                    let fi = generics::instantiate_fn(mctx, name, &type_args, call_span)?;
                    let typed_args = check_call_args(
                        &fi.ast.params,
                        &fi.decl.params,
                        args,
                        call_span,
                        fctx,
                        mctx,
                    )?;
                    let key = CalleeKey::FnInstance(generics::canonical_key(
                        InstKind::Fn,
                        name,
                        &type_args,
                    ));
                    return Ok(TypedExpr {
                        span: call_span,
                        ty: fi.decl.ret,
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
            } else if let Some(si) = mctx.structs.get(name) {
                if !si.decl.generics.is_empty() {
                    let type_args = generics::resolve_call_targs(targs, mctx)?;
                    let si = generics::instantiate_struct(mctx, name, &type_args, call_span)?;
                    return check_struct_construction(
                        name, &si, &type_args, args, call_span, fctx, mctx,
                    );
                }
            }
        }
    }
    Err(unimplemented_at("generic instantiation is", call_span))
}

fn check_entropy_call(
    targs: &[Expr],
    args: &[Arg],
    ispan: Span,
    call_span: Span,
) -> Result<TypedExpr, SemaError> {
    if targs.len() != 1 {
        return Err(type_error(
            "`entropy` needs exactly one length argument (`entropy[N]()`)".to_string(),
            ispan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            "`entropy[...]()` takes no arguments".to_string(),
            call_span,
        ));
    }
    let n_expr = &targs[0];
    let n = match n_expr {
        Expr::Int(_, text) => {
            let raw = parse_int_literal(text).ok_or_else(|| {
                type_error(
                    format!("`entropy[N]` length `{text}` is not an integer literal"),
                    ispan,
                )
            })?;
            let max = wrela_machine::machine_info::ENTROPY_LEN_MAX as i128;
            if raw < 1 || raw > max {
                return Err(type_error(
                    format!(
                        "`entropy[N]` length must be in 1..={max} (plans/M17.md freeze 1), found {raw}"
                    ),
                    ispan,
                ));
            }
            raw as u64
        }
        _ => {
            return Err(type_error(
                "`entropy[N]` needs an integer literal length".to_string(),
                ispan,
            ));
        }
    };
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Bytes(Some(Box::new(n_expr.clone()))),
        kind: TypedExprKind::Intrinsic {
            key: "entropy".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: Some(n),
            args: vec![],
        },
    })
}

pub(crate) fn image_type() -> Type {
    Type::Named("Image".to_string(), vec![])
}

pub(crate) fn image_decl_type() -> Type {
    Type::Named("ImageDecl".to_string(), vec![])
}

pub(crate) fn image_renderer_decl_type(params: Type) -> Type {
    Type::Named(
        "ImageDecl".to_string(),
        vec![TypeArg::Type(Type::Named(
            "Renderer".to_string(),
            vec![TypeArg::Type(params)],
        ))],
    )
}

fn is_image_decl_type(ty: &Type) -> bool {
    matches!(ty, Type::Named(name, _) if name == "ImageDecl")
}

type IntrinsicArgs = Vec<(String, TypedExpr)>;

pub(crate) fn check_intrinsic_args(
    args: &[Arg],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<IntrinsicArgs, SemaError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let Some(label) = &a.label else {
            return Err(type_error(
                "image builder arguments must be labeled".to_string(),
                a.span,
            ));
        };
        if !seen.insert(label.clone()) {
            return Err(type_error(
                format!("argument `{label}` bound more than once"),
                a.span,
            ));
        }
        let typed = if label == "name" {
            match &a.value {
                Expr::Name(_, pool_name)
                    if mctx.module_pools.contains(pool_name)
                        || fctx.local_pools.contains(pool_name) =>
                {
                    TypedExpr {
                        span: a.span,
                        ty: Type::Named("PoolName".to_string(), vec![]),
                        kind: TypedExprKind::PoolName(pool_name.clone()),
                    }
                }
                _ => check_expr(&a.value, None, fctx, mctx)?,
            }
        } else {
            check_expr(&a.value, None, fctx, mctx)?
        };
        out.push((label.clone(), typed));
    }
    Ok(out)
}

fn resolve_intrinsic_type_arg(e: &Expr, fctx: &FnCtx, mctx: &ModuleCtx) -> Result<Type, SemaError> {
    match e {
        Expr::Name(span, name) => {
            let ast_ty = ast::Type::Named(NamedType {
                span: *span,
                name: name.clone(),
                args: vec![],
            });
            mctx.resolve_type(&ast_ty, &fctx.local_pools)
        }
        Expr::Index(_, _, _) => resolve_intrinsic_struct_type_arg(e, mctx),
        _ => Err(unimplemented_at("generic instantiation is", e.span())),
    }
}

pub(crate) fn resolve_intrinsic_struct_type_arg(
    e: &Expr,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
    match e {
        Expr::Name(span, name) => {
            let Some(s) = mctx.structs.get(name) else {
                return Err(type_error(format!("unknown type `{name}`"), *span));
            };
            if !s.decl.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", *span));
            }
            Ok(Type::Named(name.clone(), vec![]))
        }
        Expr::Index(base, span, args) => {
            let Expr::Name(nspan, name) = base.as_ref() else {
                return Err(unimplemented_at("generic instantiation is", *span));
            };
            let Some(s) = mctx.structs.get(name) else {
                return Err(type_error(format!("unknown type `{name}`"), *nspan));
            };
            if s.decl.generics.is_empty() {
                return Err(type_error(format!("`{name}` is not generic"), *span));
            }
            let targs = generics::resolve_call_targs(args, mctx)?;
            let _ = generics::instantiate_struct(mctx, name, &targs, *span)?;
            Ok(Type::Named(name.clone(), targs))
        }
        _ => Err(unimplemented_at("generic instantiation is", e.span())),
    }
}

fn check_image_bracket_intrinsic(
    mname: &str,
    targs: &[Expr],
    args: &[Arg],
    ispan: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if targs.len() != 1 {
        return Err(type_error(
            format!("`img.{mname}` takes exactly one type argument"),
            ispan,
        ));
    }
    let type_arg = resolve_intrinsic_type_arg(&targs[0], fctx, mctx)?;
    if mname == "renderer" {
        let mut seen = BTreeSet::new();
        for arg in args {
            let Some(label) = &arg.label else {
                return Err(SemaError::at(
                    "pixels P008",
                    "renderer declaration has unknown or duplicate argument `<unlabeled>`: \
                     every renderer argument must be labeled"
                        .to_string(),
                    arg.span,
                ));
            };
            if !crate::pixels::RENDERER_LABELS.contains(&label.as_str())
                && !crate::pixels::OPTIONAL_RENDERER_LABELS.contains(&label.as_str())
            {
                return Err(SemaError::at(
                    "pixels P008",
                    format!(
                        "renderer declaration has unknown or duplicate argument `{label}`: \
                         the label is not part of the sealed renderer declaration"
                    ),
                    arg.span,
                ));
            }
            if !seen.insert(label) {
                return Err(SemaError::at(
                    "pixels P008",
                    format!(
                        "renderer declaration has unknown or duplicate argument `{label}`: \
                         the label is bound more than once"
                    ),
                    arg.span,
                ));
            }
        }
    }
    let iargs = check_intrinsic_args(args, fctx, mctx)?;
    if mname == "renderer" {
        return check_image_renderer_intrinsic(type_arg, iargs, ispan, mctx);
    }
    Ok(TypedExpr {
        span: ispan,
        ty: image_decl_type(),
        kind: TypedExprKind::Intrinsic {
            key: format!("Image.{mname}"),
            receiver: None,
            type_arg: Some(type_arg),
            const_arg: None,
            args: iargs,
        },
    })
}

fn check_image_renderer_intrinsic(
    params: Type,
    args: IntrinsicArgs,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    for actor_type in [
        "Renderer",
        "RendererWorker",
        "RenderPath",
        "RenderFrame",
        "RenderedFrame",
        "RenderError",
    ] {
        if mctx
            .type_decl_module
            .get(actor_type)
            .is_some_and(|module| !matches!(module.as_str(), "render" | "core.render"))
        {
            return Err(SemaError::at(
                "pixels P009",
                format!("`img.renderer[P]` cannot use a noncanonical type bound as `{actor_type}`"),
                span,
            ));
        }
    }
    // `Image.renderer[P]` creates a real `Renderer[P]` actor even though the
    // source expression returns an image declaration rather than a struct
    // value. Queue the concrete actor here so its initializer and methods pass
    // through the ordinary generic checker/lowering pipeline.
    let type_args = [TypeArg::Type(params.clone())];
    for name in ["RenderFrame", "RenderedFrame", "Renderer"] {
        let _ = generics::instantiate_struct(mctx, name, &type_args, span)?;
    }
    let labels: BTreeSet<&str> = args.iter().map(|(label, _)| label.as_str()).collect();
    for required in crate::pixels::RENDERER_LABELS {
        if !labels.contains(required) {
            return Err(SemaError::at(
                "pixels P008",
                format!(
                    "renderer declaration has unknown or duplicate argument `{required}`: \
                     the required argument is missing"
                ),
                span,
            ));
        }
    }
    let optional_present = labels
        .iter()
        .filter(|label| crate::pixels::OPTIONAL_RENDERER_LABELS.contains(label))
        .count();
    if labels.len() != crate::pixels::RENDERER_LABELS.len() + optional_present {
        let extra = labels
            .iter()
            .find(|label| {
                !crate::pixels::RENDERER_LABELS.contains(label)
                    && !crate::pixels::OPTIONAL_RENDERER_LABELS.contains(label)
            })
            .copied()
            .unwrap_or("<duplicate>");
        return Err(SemaError::at(
            "pixels P008",
            format!(
                "renderer declaration has unknown or duplicate argument `{extra}`: \
                 the label is not part of the sealed renderer declaration"
            ),
            span,
        ));
    }
    let root_meta = |label: &str, expected_kind: PixelsFnKind| -> Result<PixelsFnMeta, SemaError> {
        let expr = args
            .iter()
            .find(|(found, _)| found == label)
            .map(|(_, expr)| expr)
            .expect("required renderer label checked");
        let TypedExprKind::FnRef(key) = &expr.kind else {
            return Err(type_error(
                format!("`img.renderer[P]` `{label}=` must be a bare function name"),
                expr.span,
            ));
        };
        let name = key.spelling();
        let Some(info) = mctx.fns.get(&name) else {
            return Err(type_error(
                format!("renderer `{label}=` function `{name}` is not available"),
                expr.span,
            ));
        };
        let Some(mut meta) = mctx
            .pixels_fn_meta
            .borrow()
            .get(&name)
            .cloned()
            .or(check_pixels_fn_shape(&info.ast, &info.decl, mctx)?)
        else {
            return Err(type_error(
                format!(
                    "renderer `{label}=` function `{name}` lacks `@{}`",
                    match expected_kind {
                        PixelsFnKind::Field => "field",
                        PixelsFnKind::Material => "material",
                    }
                ),
                expr.span,
            ));
        };
        let declared_here = mctx
            .fn_decl_module
            .get(&name)
            .is_none_or(|module| module == &mctx.module_path);
        if meta.kind == PixelsFnKind::Field && meta.material_type.is_none() && declared_here {
            let typed = check_top_fn(&info.ast, &info.decl, mctx)?.ok_or_else(|| {
                type_error(
                    format!("renderer `field=` function `{name}` may not be generic"),
                    expr.span,
                )
            })?;
            meta.material_type = pixels_field_material_type(&typed, mctx)?;
        }
        if meta.kind != expected_kind {
            return Err(type_error(
                format!("renderer `{label}=` function `{name}` has the wrong Pixels marker"),
                expr.span,
            ));
        }
        Ok(meta)
    };
    root_meta("field", PixelsFnKind::Field)?;
    root_meta("material", PixelsFnKind::Material)?;
    Ok(TypedExpr {
        span,
        ty: image_renderer_decl_type(params.clone()),
        kind: TypedExprKind::Intrinsic {
            key: "Image.renderer".to_string(),
            receiver: None,
            type_arg: Some(params),
            const_arg: None,
            args,
        },
    })
}

fn scalar_type_by_name_expr(e: &Expr) -> Option<Type> {
    match e {
        Expr::Name(_, name) => scalar_type_by_name(name),
        _ => None,
    }
}

pub(crate) fn scalar_type_by_name(name: &str) -> Option<Type> {
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

fn is_opaque_field_name(name: &str, mctx: &ModuleCtx) -> bool {
    mctx.type_decl_module
        .get(name)
        .zip(mctx.type_decl_name.get(name))
        .is_some_and(|(module, declaration)| {
            declaration == "Field" && matches!(module.as_str(), "field" | "core.field")
        })
}

fn is_core_field_vector_type(ty: &Type, mctx: &ModuleCtx) -> bool {
    let Type::Named(name, args) = ty else {
        return false;
    };
    args.is_empty()
        && mctx
            .type_decl_module
            .get(name)
            .zip(mctx.type_decl_name.get(name))
            .is_some_and(|(module, declaration)| {
                matches!(module.as_str(), "field" | "core.field")
                    && matches!(declaration.as_str(), "Vec2" | "Vec3" | "Vec4" | "Rgb")
            })
}

fn contains_opaque_field(ty: &Type, mctx: &ModuleCtx) -> bool {
    match ty {
        Type::Named(name, args) => {
            is_opaque_field_name(name, mctx)
                || args.iter().any(|arg| match arg {
                    TypeArg::Type(ty) => contains_opaque_field(ty, mctx),
                    _ => false,
                })
        }
        Type::Array(element, _)
        | Type::Option(element)
        | Type::Static(element)
        | Type::Own(_, element) => contains_opaque_field(element, mctx),
        Type::Tuple(items) => items.iter().any(|item| contains_opaque_field(item, mctx)),
        Type::Result(ok, error) => {
            contains_opaque_field(ok, mctx) || contains_opaque_field(error, mctx)
        }
        Type::Fn(params, ret) => {
            params
                .iter()
                .any(|(_, param)| contains_opaque_field(param, mctx))
                || contains_opaque_field(ret, mctx)
        }
        _ => false,
    }
}

fn is_scalar(t: &Type) -> bool {
    is_numeric_scalar(t) || matches!(t, Type::Bool | Type::Char)
}

fn check_call_by_name(
    name: &str,
    call_span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if name == "Untrusted" {
        return Err(type_error(
            "`Untrusted[T]` is a sealed marked-value wrapper (03-hardware.md §8); it has no \
             source-visible constructor — a device control value arrives marked, and the only \
             transition out is a checked narrowing such as `.checked_le(bound)`"
                .to_string(),
            call_span,
        ));
    }
    if let Some(ty) = fctx.lookup_local(name) {
        let callee_t = TypedExpr {
            span: call_span,
            ty,
            kind: TypedExprKind::Local(name.to_string()),
        };
        return call_fn_value(callee_t, args, call_span, fctx, mctx);
    }
    if let Some(c) = mctx.consts.get(name) {
        let callee_t = TypedExpr {
            span: call_span,
            ty: c.clone(),
            kind: TypedExprKind::Const(name.to_string()),
        };
        return call_fn_value(callee_t, args, call_span, fctx, mctx);
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            let type_args = generics::infer_fn_targs(f, args, fctx, mctx, call_span)?;
            let fi = generics::instantiate_fn(mctx, name, &type_args, call_span)?;
            let typed_args =
                check_call_args(&fi.ast.params, &fi.decl.params, args, call_span, fctx, mctx)?;
            let key =
                CalleeKey::FnInstance(generics::canonical_key(InstKind::Fn, name, &type_args));
            return Ok(TypedExpr {
                span: call_span,
                ty: fi.decl.ret,
                kind: TypedExprKind::Call {
                    callee: key,
                    receiver: None,
                    args: typed_args,
                },
            });
        }
        let typed_args =
            check_call_args(&f.ast.params, &f.decl.params, args, call_span, fctx, mctx)?;
        return Ok(TypedExpr {
            span: call_span,
            ty: resolved_ret(&f.decl.ret, None, name, mctx),
            kind: TypedExprKind::Call {
                callee: CalleeKey::Fn(name.to_string()),
                receiver: None,
                args: typed_args,
            },
        });
    }
    if let Some(s) = mctx.structs.get(name) {
        if !s.decl.generics.is_empty() {
            return Err(SemaError::at(
                "generic",
                format!("`{name}` requires explicit `[Args]`"),
                call_span,
            ));
        }
        return check_struct_construction(name, s, &[], args, call_span, fctx, mctx);
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
            let it = check_expr(&args[0].value, inner_expected.as_ref(), fctx, mctx)?;
            let ty = Type::Option(Box::new(it.ty.clone()));
            Ok(TypedExpr {
                span: call_span,
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Option".to_string(),
                    variant: "Some".to_string(),
                    args: vec![TypedCallArg {
                        mode: args[0].mode,
                        value: Some(it),
                    }],
                },
            })
        }
        "Ok" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let (t_expected, e_ty) = match expected {
                Some(Type::Result(t, e)) if types::is_inferred_error_set(e) => {
                    (Some((**t).clone()), Some(Type::Never))
                }
                Some(Type::Result(t, e)) => (Some((**t).clone()), Some((**e).clone())),
                _ => (None, None),
            };
            let t_typed = check_expr(&args[0].value, t_expected.as_ref(), fctx, mctx)?;
            let e_ty = e_ty.ok_or_else(|| {
                type_error(
                    "cannot infer the error type of `Ok(...)` without context".to_string(),
                    call_span,
                )
            })?;
            let ty = Type::Result(Box::new(t_typed.ty.clone()), Box::new(e_ty));
            Ok(TypedExpr {
                span: call_span,
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    args: vec![TypedCallArg {
                        mode: args[0].mode,
                        value: Some(t_typed),
                    }],
                },
            })
        }
        "Err" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let (t_ty_opt, e_expected) = match expected {
                Some(Type::Result(t, e)) if types::is_inferred_error_set(e) => {
                    (Some((**t).clone()), None)
                }
                Some(Type::Result(t, e)) => (Some((**t).clone()), Some((**e).clone())),
                _ => (None, None),
            };
            let e_typed = check_expr(&args[0].value, e_expected.as_ref(), fctx, mctx)?;
            let t_ty = t_ty_opt.ok_or_else(|| {
                type_error(
                    "cannot infer the ok type of `Err(...)` without context".to_string(),
                    call_span,
                )
            })?;
            fctx.record_inferred_error(e_typed.ty.clone());
            let ty = Type::Result(Box::new(t_ty), Box::new(e_typed.ty.clone()));
            Ok(TypedExpr {
                span: call_span,
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Result".to_string(),
                    variant: "Err".to_string(),
                    args: vec![TypedCallArg {
                        mode: args[0].mode,
                        value: Some(e_typed),
                    }],
                },
            })
        }
        "panic" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let mt = check_expr(
                &args[0].value,
                Some(&Type::Static(Box::new(Type::Str))),
                fctx,
                mctx,
            )?;
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Never,
                kind: TypedExprKind::Panic(Box::new(mt)),
            })
        }
        "Image" => {
            let iargs = check_intrinsic_args(args, fctx, mctx)?;
            Ok(TypedExpr {
                span: call_span,
                ty: image_type(),
                kind: TypedExprKind::Intrinsic {
                    key: "Image".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
                    args: iargs,
                },
            })
        }
        "now" => {
            if !args.is_empty() {
                return Err(type_error(
                    "`now` takes no arguments".to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Named("Instant".to_string(), vec![]),
                kind: TypedExprKind::Intrinsic {
                    key: "now".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
                    args: vec![],
                },
            })
        }
        "InterruptCell" => check_interrupt_cell_new(args, expected, call_span, fctx, mctx),
        "wake" => check_wake_call(args, call_span, fctx, mctx),
        _ => {
            capability_forgery_check(name, "called", call_span)?;
            Err(type_error(format!("`{name}` is not callable"), call_span))
        }
    }
}

fn capability_forgery_check(name: &str, attempt: &str, span: Span) -> Result<(), SemaError> {
    if !crate::sema::classes::name_holds_authority(name) {
        return Ok(());
    }
    let kind = crate::eval::image_checks::sealed_authority_kind(name);
    let origin = if crate::eval::image_checks::is_protocol_state_type_name(name) {
        "a bring-up state is produced only by the sealed transport's own transitions"
    } else {
        "a capability is minted only where the image binds a declared device to a `@driver`"
    };
    Err(type_error(
        format!(
            "`{name}` is {kind} and cannot be {attempt}: its constructor is not source-visible, \
             and {origin}"
        ),
        span,
    ))
}

fn capability_name_in_type_expr(e: &Expr) -> Option<&str> {
    let name = match e {
        Expr::Name(_, n) => n.as_str(),
        Expr::Index(base, _, _) => match base.as_ref() {
            Expr::Name(_, n) => n.as_str(),
            _ => return None,
        },
        _ => return None,
    };
    crate::sema::classes::name_holds_authority(name).then_some(name)
}

fn check_method_generic_call(
    receiver_ty: &Type,
    method: &str,
    d: &types::DeclFn,
    args: &[Arg],
    explicit_targs: Option<&[Expr]>,
    call_span: Span,
    call_receiver: Option<TypedExpr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let type_args = match explicit_targs {
        Some(targs) => generics::resolve_call_targs(targs, mctx)?,
        None => generics::infer_method_targs(method, d, args, fctx, mctx, call_span)?,
    };
    let (ast, decl) =
        generics::instantiate_method(mctx, receiver_ty, method, &type_args, call_span)?;
    let typed_args = check_call_args(&ast.params, &decl.params, args, call_span, fctx, mctx)?;
    let key = CalleeKey::FnInstance(generics::canonical_method_key(
        receiver_ty,
        method,
        &type_args,
    ));
    Ok(TypedExpr {
        span: call_span,
        ty: decl.ret,
        kind: TypedExprKind::Call {
            callee: key,
            receiver: call_receiver.map(Box::new),
            args: typed_args,
        },
    })
}

fn check_call_by_field(
    base: &Expr,
    fspan: Span,
    name: &str,
    call_span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            if bname == "VirtQueue" && name == "configure" {
                return check_virtqueue_configure(args, fspan, call_span, fctx, mctx);
            }
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", call_span));
                }
                if let Some((af, d)) = s.assoc_fn(name) {
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(bname.clone(), vec![]);
                        return check_method_generic_call(
                            &recv_ty, name, d, args, None, call_span, None, fctx, mctx,
                        );
                    }
                    let typed_args =
                        check_call_args(&af.params, &d.params, args, call_span, fctx, mctx)?;
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        span: call_span,
                        ty: resolved_ret(&d.ret, Some(bname), name, mctx),
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
                if name == "claim" {
                    return check_device_claim(bname, args, fspan, call_span, fctx, mctx);
                }
                return Err(type_error(
                    format!("type `{bname}` has no associated function `{name}`"),
                    fspan,
                ));
            }
            if let Some(e) = mctx.enums.get(bname.as_str()) {
                if let Some((af, d)) = e.assoc_fn(name) {
                    if !e.generics.is_empty() {
                        return Err(unimplemented_at("generic instantiation is", call_span));
                    }
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(bname.clone(), vec![]);
                        return check_method_generic_call(
                            &recv_ty, name, d, args, None, call_span, None, fctx, mctx,
                        );
                    }
                    let typed_args =
                        check_call_args(&af.params, &d.params, args, call_span, fctx, mctx)?;
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        span: call_span,
                        ty: resolved_ret(&d.ret, Some(bname), name, mctx),
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
                if e.variants.iter().any(|v| v.name == name) {
                    let (targs, decl) =
                        resolve_enum_for_variant_construction(bname, e, expected, call_span, mctx)?;
                    let dv = decl
                        .variants
                        .iter()
                        .find(|v| v.name == name)
                        .expect("name membership checked above");
                    let payload_types = decl_variant_payload_types(dv);
                    let typed_args =
                        check_variant_args(&payload_types, args, call_span, fctx, mctx)?;
                    let ty = Type::Named(bname.clone(), targs);
                    if contains_opaque_field(&ty, mctx) {
                        return Err(type_error(
                            "P004: opaque `Field` may not be stored in an enum value".to_string(),
                            call_span,
                        ));
                    }
                    return Ok(TypedExpr {
                        span: call_span,
                        ty,
                        kind: TypedExprKind::EnumConstruct {
                            enum_name: bname.clone(),
                            variant: name.to_string(),
                            args: typed_args,
                        },
                    });
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant or associated function `{name}`"),
                    fspan,
                ));
            }
        }
    }
    if let Expr::Field(inner, _, register) = base {
        if let Ok(mmio) = check_expr(inner, None, fctx, mctx) {
            if let Type::Named(cap, targs) = &unwrap_own(mmio.ty.clone()) {
                if cap == "Mmio" {
                    return check_mmio_access(
                        mmio.clone(),
                        targs,
                        register,
                        name,
                        args,
                        fspan,
                        call_span,
                        fctx,
                        mctx,
                    );
                }
            }
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    if let Type::Named(state, targs) = &base_ty {
        if crate::eval::image_checks::is_protocol_state_type_name(state) {
            return check_device_state_call(
                base_t.clone(),
                state,
                targs,
                name,
                args,
                fspan,
                call_span,
                fctx,
                mctx,
            );
        }
    }
    if let Type::Named(marker, targs) = &base_ty {
        if marker == "Untrusted" {
            return check_untrusted_narrowing(
                base_t, targs, name, args, fspan, call_span, fctx, mctx,
            );
        }
    }
    if let Type::Named(q, _) = &base_ty {
        if q == "VirtQueue" {
            return check_virtqueue_method(
                base_t, name, args, fspan, call_span, expected, fctx, mctx,
            );
        }
    }
    if let Type::Named(cap, targs) = &base_ty {
        if cap == "Mmio" {
            return Err(type_error(
                format!(
                    "`{}` has no method `{name}`; a typed register map is used only through a \
                     declared register — `<mmio>.<register>.read()` or \
                     `<mmio>.<register>.write(v)` (03-hardware.md §2){}",
                    types::render_type(&base_ty),
                    mmio_register_hint(targs, mctx),
                ),
                fspan,
            ));
        }
    }
    if let Type::Named(cap, _) = &base_ty {
        if cap == "IrqCap" {
            return check_irq_cap_call(base_t, name, args, fspan, call_span, fctx, mctx);
        }
    }
    if let Type::Named(cell, _) = &base_ty {
        if cell == "InterruptCell" {
            return check_interrupt_cell_call(base_t, name, args, fspan, call_span, fctx, mctx);
        }
    }
    if let Type::Named(outer, _) = &base_ty {
        if outer == "Actor" {
            return Err(type_error(
                format!(
                    "calling `{name}` through an `Actor[T]` handle requires `await` or `send`, \
                     not a bare call"
                ),
                call_span,
            ));
        }
        if outer == "Group" {
            return match name {
                "start" => check_group_start(base_t, args, call_span, fctx, mctx),
                "join_all" => Err(type_error(
                    "`join_all` must be `await`ed".to_string(),
                    call_span,
                )),
                other => Err(type_error(
                    format!("`Group` has no method `{other}`"),
                    fspan,
                )),
            };
        }
    }
    if base_ty == image_type() {
        return check_image_method_intrinsic(name, args, call_span, fctx, mctx);
    }
    if is_image_decl_type(&base_ty) {
        return check_image_decl_method_intrinsic(base_t, name, args, fspan, call_span, fctx, mctx);
    }
    if let Type::Array(elem, len) = &base_ty {
        if name == "map_take" || name == "try_map_take" {
            return check_array_map_take(
                base_t, name, elem, len, args, fspan, call_span, fctx, mctx,
            );
        }
        if name == "each" || name == "each_mut" {
            return Err(type_error(
                format!(
                    "`[{}; N]` has no method `{name}`; lent array iteration is \
                     `List[T, ..N].each` / `.each_mut` (05-library.md §7)",
                    types::render_type(elem)
                ),
                fspan,
            ));
        }
    }
    match &base_ty {
        Type::Named(sname, targs) => {
            if let Some(s) = if targs.is_empty() {
                mctx.structs
                    .get(sname.as_str())
                    .map(std::borrow::Cow::Borrowed)
            } else if mctx.structs.contains_key(sname.as_str()) {
                Some(std::borrow::Cow::Owned(generics::instantiate_struct(
                    mctx, sname, targs, call_span,
                )?))
            } else {
                None
            } {
                let Some((mf, d)) = s.method(name) else {
                    return Err(missing_method_error(
                        format!("type `{sname}` has no method `{name}`"),
                        sname,
                        name,
                        fspan,
                    ));
                };
                if !d.generics.is_empty() {
                    let recv_ty = Type::Named(sname.clone(), targs.clone());
                    return check_method_generic_call(
                        &recv_ty,
                        name,
                        d,
                        args,
                        None,
                        call_span,
                        Some(base_t),
                        fctx,
                        mctx,
                    );
                }
                let typed_args =
                    check_call_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
                let key = if targs.is_empty() {
                    CalleeKey::Method(sname.clone(), name.to_string())
                } else {
                    CalleeKey::MethodInstance(
                        generics::canonical_key(InstKind::Struct, sname, targs),
                        name.to_string(),
                    )
                };
                return Ok(TypedExpr {
                    span: call_span,
                    ty: resolved_ret(&d.ret, Some(sname), name, mctx),
                    kind: TypedExprKind::Call {
                        callee: key,
                        receiver: Some(Box::new(base_t)),
                        args: typed_args,
                    },
                });
            }
            if targs.is_empty() {
                if let Some(e) = mctx.enums.get(sname.as_str()) {
                    let Some((mf, d)) = e.method(name) else {
                        return Err(missing_method_error(
                            format!("type `{sname}` has no method `{name}`"),
                            sname,
                            name,
                            fspan,
                        ));
                    };
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(sname.clone(), vec![]);
                        return check_method_generic_call(
                            &recv_ty,
                            name,
                            d,
                            args,
                            None,
                            call_span,
                            Some(base_t),
                            fctx,
                            mctx,
                        );
                    }
                    let typed_args =
                        check_call_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
                    return Ok(TypedExpr {
                        span: call_span,
                        ty: resolved_ret(&d.ret, Some(sname), name, mctx),
                        kind: TypedExprKind::Call {
                            callee: CalleeKey::Method(sname.clone(), name.to_string()),
                            receiver: Some(Box::new(base_t)),
                            args: typed_args,
                        },
                    });
                }
            } else if mctx.enums.contains_key(sname.as_str()) {
                return Err(unimplemented_at("generic instantiation is", call_span));
            }
            Err(missing_method_error(
                format!("type `{sname}` has no method `{name}`"),
                sname,
                name,
                fspan,
            ))
        }
        other => {
            if name == "format" {
                if !args.is_empty() {
                    return Err(type_error(
                        format!("too many arguments, expected 0, found {}", args.len()),
                        call_span,
                    ));
                }
                if let Some(k) = types::scalar_format_bound(other) {
                    return Ok(TypedExpr {
                        span: call_span,
                        ty: Type::String(Box::new(Expr::Int(call_span, k.to_string()))),
                        kind: TypedExprKind::Call {
                            callee: CalleeKey::Method(
                                types::render_type(other),
                                "format".to_string(),
                            ),
                            receiver: Some(Box::new(base_t)),
                            args: vec![],
                        },
                    });
                }
            }
            let type_name = types::render_type(other);
            Err(missing_method_error(
                format!("type `{type_name}` has no method `{name}`"),
                &type_name,
                name,
                fspan,
            ))
        }
    }
}

fn mmio_register_hint(targs: &[types::TypeArg], mctx: &ModuleCtx) -> String {
    let Some(layout) = mmio_layout_of(targs, mctx) else {
        return String::new();
    };
    let names = types::mmio_register_names(layout);
    if names.is_empty() {
        return String::new();
    }
    format!(
        "; `{}` declares {}",
        layout.name,
        names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn mmio_layout_of<'a>(
    targs: &[types::TypeArg],
    mctx: &'a ModuleCtx,
) -> Option<&'a types::LayoutType> {
    match targs.first() {
        Some(types::TypeArg::Type(Type::Named(l, _))) => mctx.layouts.get(l.as_str()),
        _ => None,
    }
}

fn mmio_bare_selection_error(
    targs: &[types::TypeArg],
    name: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> SemaError {
    let layout = mmio_layout_of(targs, mctx);
    let known = layout.is_some_and(|l| types::mmio_register(l, name).is_some());
    if !known {
        return type_error(
            format!(
                "`{}` declares no register `{name}`{}",
                layout
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| "this `@layout(mmio)` type".to_string()),
                mmio_register_hint(targs, mctx),
            ),
            span,
        );
    }
    type_error(
        format!(
            "register `{}.{name}` is not a value; an MMIO register exists only as an access — \
             write `.read()` or `.write(v)` (03-hardware.md §2)",
            layout.map(|l| l.name.as_str()).unwrap_or("?"),
        ),
        span,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_mmio_access(
    mmio: TypedExpr,
    targs: &[types::TypeArg],
    register: &str,
    op: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some(layout) = mmio_layout_of(targs, mctx) else {
        return Err(type_error(
            format!(
                "`{}`'s layout is not a declared `@layout(mmio)` type (03-hardware.md §2)",
                types::render_type(&mmio.ty)
            ),
            fspan,
        ));
    };
    let Some(reg) = types::mmio_register(layout, register) else {
        return Err(type_error(
            format!(
                "`{}` declares no register `{register}`{}",
                layout.name,
                mmio_register_hint(targs, mctx)
            ),
            fspan,
        ));
    };
    if !matches!(op, "read" | "write") {
        return Err(type_error(
            format!(
                "register `{}.{register}` has no operation `{op}`; a declared MMIO register is \
                 read with `.read()` or written with `.write(v)` (03-hardware.md §2)",
                layout.name
            ),
            fspan,
        ));
    }

    let declared = format!("{}.{register}: {}", layout.name, register_type_text(&reg));
    match (reg.direction, op) {
        (Some(types::MmioDirection::ReadOnly), "read")
        | (Some(types::MmioDirection::WriteOnly), "write") => {}
        (Some(types::MmioDirection::WriteOnly), _) => {
            return Err(type_error(
                format!(
                    "register `{declared}` is write-only and cannot be read (03-hardware.md §2: a \
                     register's declared direction governs its access)"
                ),
                call_span,
            ));
        }
        (Some(types::MmioDirection::ReadOnly), _) => {
            return Err(type_error(
                format!(
                    "register `{declared}` is read-only and cannot be written (03-hardware.md §2: \
                     a register's declared direction governs its access)"
                ),
                call_span,
            ));
        }
        (None, _) => {
            return Err(type_error(
                format!(
                    "register `{declared}` declares no direction, so it has neither a `read()` \
                     nor a `write(v)`; a register map's fields are `ReadOnly[T]` or `WriteOnly[T]` \
                     (03-hardware.md §2)"
                ),
                call_span,
            ));
        }
    }

    if layout.endian != types::LayoutEndian::Little {
        return Err(unimplemented_at(
            &format!(
                "an access to `{declared}`, whose `@layout(mmio, endian={})` disagrees with this \
                 target's little-endian ABI (06-machine.md §2) and would need a byte swap that is \
                 not emitted, is",
                layout.endian.as_str()
            ),
            call_span,
        ));
    }

    let Some(scalar) = scalar_type_by_name(&reg.scalar) else {
        return Err(type_error(
            format!("register `{declared}` has no scalar register type (03-hardware.md §2)"),
            fspan,
        ));
    };

    let mut intrinsic_args = vec![(
        "register".to_string(),
        TypedExpr {
            span: call_span,
            ty: Type::Static(Box::new(Type::Str)),
            kind: TypedExprKind::Str(register.to_string()),
        },
    )];
    let ty = match op {
        "read" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!("`{}.{register}.read()` takes no arguments", layout.name),
                    call_span,
                ));
            }
            scalar.clone()
        }
        _ => {
            let [arg] = args else {
                return Err(type_error(
                    format!(
                        "`{}.{register}.write(v)` takes exactly one argument, the {} value to \
                         write; found {}",
                        layout.name,
                        types::render_type(&scalar),
                        args.len()
                    ),
                    call_span,
                ));
            };
            if let Some(label) = &arg.label {
                return Err(type_error(
                    format!(
                        "`{}.{register}.write(v)`'s value is positional; `{label}=` names no \
                         parameter",
                        layout.name
                    ),
                    arg.span,
                ));
            }
            let value = check_expr(&arg.value, Some(&scalar), fctx, mctx)?;
            intrinsic_args.push(("value".to_string(), value));
            Type::Unit
        }
    };

    Ok(TypedExpr {
        span: call_span,
        ty,
        kind: TypedExprKind::Intrinsic {
            key: format!("Mmio.{op}"),
            receiver: Some(Box::new(mmio)),
            type_arg: Some(scalar),
            const_arg: None,
            args: intrinsic_args,
        },
    })
}

fn register_type_text(reg: &types::MmioRegister) -> String {
    match reg.direction {
        Some(d) => format!("{}[{}]", d.wrapper(), reg.scalar),
        None => reg.scalar.clone(),
    }
}

pub fn is_mmio_access_intrinsic(key: &str) -> bool {
    matches!(key, "Mmio.read" | "Mmio.write")
}

pub(crate) fn is_untrusted_type(ty: &Type) -> bool {
    matches!(ty, Type::Named(name, _) if name == "Untrusted")
}

pub(crate) fn untrusted_use_error(use_kind: &str, span: Span) -> SemaError {
    type_error(
        format!(
            "`Untrusted[T]` cannot be used as {use_kind} until checked-narrowed — write \
             `.checked_le(bound)` (03-hardware.md §8)"
        ),
        span,
    )
}

fn untrusted_coercion_message(expected: &Type, found: &Type) -> Option<String> {
    if !is_untrusted_type(found) {
        return None;
    }
    if is_untrusted_type(expected) {
        return None;
    }
    Some(format!(
        "`Untrusted[T]` cannot be used as a plain `{}` until checked-narrowed — write \
         `.checked_le(bound)` (03-hardware.md §8); expected `{}`, found `{}`",
        types::render_type(expected),
        types::render_type(expected),
        types::render_type(found),
    ))
}

pub fn is_untrusted_narrowing_intrinsic(key: &str) -> bool {
    key == "Untrusted.checked_le"
}

#[allow(clippy::too_many_arguments)]
fn check_untrusted_narrowing(
    receiver: TypedExpr,
    targs: &[types::TypeArg],
    name: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some(types::TypeArg::Type(inner)) = targs.first() else {
        return Err(type_error(
            "`Untrusted` with no payload type argument".to_string(),
            fspan,
        ));
    };
    if !is_integer_scalar(inner) {
        return Err(type_error(
            format!(
                "`Untrusted[{}]` cannot be checked-narrowed: the payload must be an integer \
                 scalar (03-hardware.md §8's `Untrusted[usize]` worked example)",
                types::render_type(inner)
            ),
            fspan,
        ));
    }
    if name != "checked_le" {
        if name.starts_with("checked_") {
            return Err(unimplemented_at(
                &format!(
                    "`Untrusted[T].{name}` (03-hardware.md §8 spells only `.checked_le(bound)`; \
                     any other checked narrowing is"
                ),
                call_span,
            ));
        }
        return Err(type_error(
            format!(
                "`Untrusted[{}]` has no method `{name}`; the only source-visible transition is \
                 `.checked_le(bound)` (03-hardware.md §8)",
                types::render_type(inner)
            ),
            fspan,
        ));
    }
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`Untrusted[{}].checked_le(bound)` takes exactly one argument; found {}",
                types::render_type(inner),
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        if label != "bound" {
            return Err(type_error(
                format!(
                    "`Untrusted[{}].checked_le(bound)`'s argument is positional or `bound=`; \
                     `{label}=` names no parameter",
                    types::render_type(inner)
                ),
                arg.span,
            ));
        }
    }
    let bound = check_expr(&arg.value, Some(inner), fctx, mctx)?;
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Result(Box::new(inner.clone()), Box::new(Type::Unit)),
        kind: TypedExprKind::Intrinsic {
            key: "Untrusted.checked_le".to_string(),
            receiver: Some(Box::new(receiver)),
            type_arg: Some(inner.clone()),
            const_arg: None,
            args: vec![("bound".to_string(), bound)],
        },
    })
}

use super::transport::*;
pub use super::transport::{
    is_device_transport_intrinsic, is_interrupt_cell_intrinsic, is_interrupt_cell_type,
    is_irq_cap_intrinsic, is_queue_op_deferred, is_queue_op_intrinsic, is_wake_intrinsic,
};

fn check_irq_cap_call(
    irq: TypedExpr,
    method: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let rendered = types::render_type(&irq.ty);
    match method {
        "bind" => check_irq_bind(irq, &rendered, args, fspan, call_span, fctx, mctx),
        "unmask" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!(
                        "`{rendered}.unmask()` takes no arguments; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            }
            let _ = fspan;
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "IrqCap.unmask".to_string(),
                    receiver: Some(Box::new(irq)),
                    type_arg: None,
                    const_arg: None,
                    args: Vec::new(),
                },
            })
        }
        other => Err(type_error(
            format!(
                "`{rendered}` has no method `{other}`; 03-hardware.md §6 gives an `IrqCap` \
                 `bind(handler)` and `unmask()`"
            ),
            fspan,
        )),
    }
}

fn check_irq_bind(
    irq: TypedExpr,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{rendered}.bind(handler)` takes exactly one argument, the ISR to bind \
                 (03-hardware.md §6: `irq.bind(self.on_queue_irq)`); found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!("`bind(handler)`'s handler is positional; `{label}=` names no parameter"),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`bind(handler)`'s argument is a method reference, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let handler = resolve_irq_bind_handler(&arg.value, arg.span, fctx, mctx)?;
    let _ = fspan;
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "IrqCap.bind".to_string(),
            receiver: Some(Box::new(irq)),
            type_arg: None,
            const_arg: None,
            args: vec![("handler".to_string(), handler)],
        },
    })
}

fn resolve_irq_bind_handler(
    expr: &Expr,
    span: Span,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Expr::Field(base, _, method) = expr else {
        return Err(type_error(
            "`IrqCap.bind`'s handler is a method reference \
             (`self.on_queue_irq` or `Driver.on_queue_irq` — 03-hardware.md §6)"
                .to_string(),
            span,
        ));
    };
    if let Expr::Name(_, name) = base.as_ref() {
        if name == "self" {
            let Some(self_ty) = fctx.lookup_local("self") else {
                return Err(type_error(
                    "`self.on_queue_irq` is only meaningful inside a method with a `self` receiver"
                        .to_string(),
                    span,
                ));
            };
            let Type::Named(sname, targs) = unwrap_own(self_ty.clone()) else {
                return Err(type_error(
                    format!(
                        "`IrqCap.bind`'s handler must name a method of a `@driver`; `self` has type \
                         `{}`",
                        types::render_type(&self_ty)
                    ),
                    span,
                ));
            };
            return irq_handler_fnref(&sname, &targs, method, span, mctx);
        }
        if let Some(s) = mctx.structs.get(name.as_str()) {
            if s.method(method).is_some() {
                return irq_handler_fnref(name, &[], method, span, mctx);
            }
            if s.assoc_fn(method).is_some() {
                return irq_handler_fnref(name, &[], method, span, mctx);
            }
            return Err(type_error(
                format!("type `{name}` has no method `{method}` to bind as an ISR"),
                span,
            ));
        }
    }
    Err(type_error(
        "`IrqCap.bind`'s handler is a method reference \
         (`self.on_queue_irq` or `Driver.on_queue_irq` — 03-hardware.md §6)"
            .to_string(),
        span,
    ))
}

fn irq_handler_fnref(
    struct_name: &str,
    targs: &[types::TypeArg],
    method: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let owned;
    let s: &StructInfo = if targs.is_empty() {
        let Some(s) = mctx.structs.get(struct_name) else {
            return Err(type_error(
                format!("type `{struct_name}` is not a declared struct"),
                span,
            ));
        };
        s
    } else {
        owned = generics::instantiate_struct(mctx, struct_name, targs, span)?;
        &owned
    };
    if !s.decl.is_driver {
        return Err(type_error(
            format!(
                "`IrqCap.bind` binds an ISR of a `@driver`; `{struct_name}` is not a `@driver` \
                 (03-hardware.md §6)"
            ),
            span,
        ));
    }
    let Some((_, d)) = s.method(method).or_else(|| s.assoc_fn(method)) else {
        return Err(type_error(
            format!("`@driver` `{struct_name}` has no method `{method}` to bind as an ISR"),
            span,
        ));
    };
    if !d.params.is_empty() {
        return Err(type_error(
            format!(
                "ISR `{struct_name}.{method}` must take no parameters beyond `self` \
                 (03-hardware.md §6's worked `on_queue_irq(self)`); found {} parameter(s)",
                d.params.len()
            ),
            span,
        ));
    }
    if d.ret != Type::Unit {
        return Err(type_error(
            format!(
                "ISR `{struct_name}.{method}` must return `unit` (03-hardware.md §6); found `{}`",
                types::render_type(&d.ret)
            ),
            span,
        ));
    }
    if d.is_async {
        return Err(type_error(
            format!(
                "ISR `{struct_name}.{method}` must be a plain `fn`, not `async fn` \
                 (03-hardware.md §6: an interrupt handler is a plain `fn`)"
            ),
            span,
        ));
    }
    let key = if targs.is_empty() {
        CalleeKey::Method(struct_name.to_string(), method.to_string())
    } else {
        CalleeKey::MethodInstance(
            generics::canonical_key(InstKind::Struct, struct_name, targs),
            method.to_string(),
        )
    };
    Ok(TypedExpr {
        span: span,
        ty: fn_value_type(d),
        kind: TypedExprKind::FnRef(key),
    })
}

fn interrupt_cell_elem_ty(cell_ty: &Type, span: Span) -> Result<&Type, SemaError> {
    match cell_ty {
        Type::Named(n, targs) if n == "InterruptCell" => match targs.first() {
            Some(types::TypeArg::Type(inner)) => Ok(inner),
            _ => Err(type_error(
                "`InterruptCell` is missing its element type".to_string(),
                span,
            )),
        },
        _ => Err(type_error(
            format!(
                "expected an `InterruptCell[T]`, found `{}`",
                types::render_type(cell_ty)
            ),
            span,
        )),
    }
}

fn require_interrupt_cell_u32(elem: &Type, span: Span) -> Result<(), SemaError> {
    if matches!(elem, Type::U32) {
        return Ok(());
    }
    Err(type_error(
        format!(
            "`InterruptCell[{}]` is not supported yet — revision 0.1 admits only \
             `InterruptCell[u32]` (03-hardware.md §6's worked example; plans/M7.md item G)",
            types::render_type(elem)
        ),
        span,
    ))
}

fn check_interrupt_cell_new(
    args: &[Arg],
    expected: Option<&Type>,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`InterruptCell(value)` takes exactly one argument; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!(
                "`InterruptCell(value)`'s argument is positional; `{label}=` names no parameter"
            ),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`InterruptCell(value)` takes a plain value; drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let elem_expected = match expected {
        Some(Type::Named(n, targs)) if n == "InterruptCell" => match targs.first() {
            Some(types::TypeArg::Type(inner)) => Some(inner.clone()),
            _ => None,
        },
        _ => Some(Type::U32),
    };
    let value = check_expr(&arg.value, elem_expected.as_ref(), fctx, mctx)?;
    require_interrupt_cell_u32(&value.ty, arg.span)?;
    let ty = Type::Named(
        "InterruptCell".to_string(),
        vec![types::TypeArg::Type(value.ty.clone())],
    );
    Ok(TypedExpr {
        span: call_span,
        ty,
        kind: TypedExprKind::Intrinsic {
            key: "InterruptCell.new".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
            args: vec![("value".to_string(), value)],
        },
    })
}

fn check_interrupt_cell_call(
    cell: TypedExpr,
    method: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let rendered = types::render_type(&cell.ty);
    let elem = interrupt_cell_elem_ty(&cell.ty, fspan)?;
    require_interrupt_cell_u32(elem, fspan)?;
    let elem_ty = elem.clone();
    match method {
        "load_acquire" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!(
                        "`{rendered}.load_acquire()` takes no arguments; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                span: call_span,
                ty: elem_ty,
                kind: TypedExprKind::Intrinsic {
                    key: "InterruptCell.load_acquire".to_string(),
                    receiver: Some(Box::new(cell)),
                    type_arg: None,
                    const_arg: None,
                    args: Vec::new(),
                },
            })
        }
        "store_release" | "swap_acquire" | "fetch_or_release" => {
            let [arg] = args else {
                return Err(type_error(
                    format!(
                        "`{rendered}.{method}(value)` takes exactly one argument; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            };
            if let Some(label) = &arg.label {
                return Err(type_error(
                    format!(
                        "`{method}(value)`'s argument is positional; `{label}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            if arg.mode != AccessMode::Read {
                return Err(type_error(
                    format!(
                        "`{method}(value)` takes a plain value; drop the `{}`",
                        arg.mode.as_str()
                    ),
                    arg.span,
                ));
            }
            let value = check_expr(&arg.value, Some(&elem_ty), fctx, mctx)?;
            let ret_ty = if method == "store_release" {
                Type::Unit
            } else {
                elem_ty
            };
            Ok(TypedExpr {
                span: call_span,
                ty: ret_ty,
                kind: TypedExprKind::Intrinsic {
                    key: format!("InterruptCell.{method}"),
                    receiver: Some(Box::new(cell)),
                    type_arg: None,
                    const_arg: None,
                    args: vec![("value".to_string(), value)],
                },
            })
        }
        other => Err(type_error(
            format!(
                "`{rendered}` has no method `{other}`; 03-hardware.md §6 gives an `InterruptCell` \
                 `load_acquire()`, `store_release(v)`, `swap_acquire(v)`, and `fetch_or_release(v)`"
            ),
            fspan,
        )),
    }
}

fn check_array_map_take(
    base_t: TypedExpr,
    name: &str,
    elem: &Type,
    len: &Expr,
    args: &[Arg],
    _fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 1 {
        return Err(arity_error(1, args.len(), call_span));
    }
    let a = &args[0];
    if a.label.is_some() {
        return Err(type_error(
            format!("`{name}`'s mapper argument must not be labeled"),
            a.span,
        ));
    }
    if a.mode != AccessMode::Read {
        return Err(type_error(
            format!("`{name}`'s mapper is passed unmarked (a function value)"),
            a.span,
        ));
    }
    if matches!(a.value, Expr::Closure(_)) {
        return Err(type_error(
            format!(
                "`{name}` takes a named function today (`fn(take T) -> ...`); \
                 a closure mapper needs return-type inference (plans/M9.md item F3)"
            ),
            a.span,
        ));
    }
    let mapper = check_expr(&a.value, None, fctx, mctx)?;
    let Type::Fn(params, ret) = &mapper.ty else {
        return Err(type_error(
            format!(
                "`{name}` expects a function value, found `{}`",
                types::render_type(&mapper.ty)
            ),
            a.span,
        ));
    };
    if params.len() != 1 || params[0].0 != AccessMode::Take || !types_eq(&params[0].1, elem) {
        return Err(type_error(
            format!(
                "`{name}` expects `fn(take {}) -> ...`, found `{}`",
                types::render_type(elem),
                types::render_type(&mapper.ty)
            ),
            a.span,
        ));
    }
    match name {
        "map_take" => Ok(TypedExpr {
            span: call_span,
            ty: Type::Array(Box::new((**ret).clone()), Box::new(len.clone())),
            kind: TypedExprKind::Intrinsic {
                key: "Array.map_take".to_string(),
                receiver: Some(Box::new(base_t)),
                type_arg: None,
                const_arg: None,
                args: vec![("mapper".to_string(), mapper)],
            },
        }),
        "try_map_take" => {
            let Type::Result(ok, err) = ret.as_ref() else {
                return Err(type_error(
                    format!(
                        "`try_map_take` expects `fn(take {}) -> Result[U, E]`, found `{}`",
                        types::render_type(elem),
                        types::render_type(&mapper.ty)
                    ),
                    a.span,
                ));
            };
            if is_resource_type(elem, mctx) || is_resource_type(ok, mctx) {
                return Err(type_error(
                    "`try_map_take` requires auto-reclaimable (data) element types; \
                     protocol resources need an explicit loop (05-library.md §7)"
                        .to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Result(
                    Box::new(Type::Array(Box::new((**ok).clone()), Box::new(len.clone()))),
                    Box::new((**err).clone()),
                ),
                kind: TypedExprKind::Intrinsic {
                    key: "Array.try_map_take".to_string(),
                    receiver: Some(Box::new(base_t)),
                    type_arg: None,
                    const_arg: None,
                    args: vec![("mapper".to_string(), mapper)],
                },
            })
        }
        _ => unreachable!(),
    }
}

fn check_wake_call(
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`wake(task)` takes exactly one argument, a statically bound `@task` method \
                 (03-hardware.md §6: `wake(BlkDriver.drain_used)`); found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!("`wake(task)`'s argument is positional; `{label}=` names no parameter"),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`wake(task)`'s argument is a method reference, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let target = resolve_wake_target(&arg.value, arg.span, fctx, mctx)?;
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "wake".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
            args: vec![("task".to_string(), target)],
        },
    })
}

fn resolve_wake_target(
    expr: &Expr,
    span: Span,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let handler = resolve_irq_bind_handler(expr, span, fctx, mctx).map_err(|e| {
        if e.message.contains("IrqCap.bind") {
            SemaError {
                message: e
                    .message
                    .replace("`IrqCap.bind`'s handler", "`wake`'s task")
                    .replace("to bind as an ISR", "to wake as a bottom half"),
                ..e
            }
        } else {
            e
        }
    })?;
    let TypedExprKind::FnRef(key) = &handler.kind else {
        return Err(type_error(
            "`wake`'s task must be a method reference (`Driver.drain_used`)".to_string(),
            span,
        ));
    };
    let (sname, method): (String, String) = match key {
        CalleeKey::Method(s, m) => (s.clone(), m.clone()),
        CalleeKey::MethodInstance(ikey, m) => {
            let bare = ikey
                .strip_prefix("struct:")
                .unwrap_or(ikey.as_str())
                .split('[')
                .next()
                .unwrap_or(ikey.as_str());
            (bare.to_string(), m.clone())
        }
        _ => {
            return Err(type_error(
                "`wake`'s task must name a `@driver` method".to_string(),
                span,
            ));
        }
    };
    let Some(s) = mctx.structs.get(sname.as_str()) else {
        return Err(type_error(
            format!("type `{sname}` is not a declared struct"),
            span,
        ));
    };
    let Some((_, d)) = s.method(&method) else {
        return Err(type_error(
            format!("`@driver` `{sname}` has no method `{method}` to wake"),
            span,
        ));
    };
    if !d.is_task {
        return Err(type_error(
            format!(
                "`wake` requires a statically bound `@task` (03-hardware.md §6); \
                 `{sname}.{method}` is not marked `@task`"
            ),
            span,
        ));
    }
    Ok(handler)
}

pub(crate) use super::actor::*;

pub(crate) fn check_image_method_intrinsic(
    name: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match name {
        "driver" | "actor" => {
            let Some(first) = args.first() else {
                return Err(type_error(
                    format!("`img.{name}` needs a leading type argument"),
                    call_span,
                ));
            };
            if first.label.is_some() {
                return Err(type_error(
                    format!("`img.{name}`'s leading argument must not be labeled"),
                    first.span,
                ));
            }
            let type_arg = resolve_intrinsic_struct_type_arg(&first.value, mctx)?;
            let iargs = check_intrinsic_args(&args[1..], fctx, mctx)?;
            Ok(TypedExpr {
                span: call_span,
                ty: image_decl_type(),
                kind: TypedExprKind::Intrinsic {
                    key: format!("Image.{name}"),
                    receiver: None,
                    type_arg: Some(type_arg),
                    const_arg: None,
                    args: iargs,
                },
            })
        }
        "on_failure" => {
            let iargs = check_intrinsic_args(args, fctx, mctx)?;
            if !iargs.iter().any(|(l, _)| l == "policy") {
                return Err(type_error(
                    "`img.on_failure` requires `policy`".to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "Image.on_failure".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
                    args: iargs,
                },
            })
        }
        "check_layout" => {
            if args.len() != 1 || args[0].label.is_some() {
                return Err(type_error(
                    "`img.check_layout` takes exactly one positional argument".to_string(),
                    call_span,
                ));
            }
            let f = check_expr(&args[0].value, None, fctx, mctx)?;
            match &f.kind {
                TypedExprKind::FnRef(key) => {
                    let name = key.spelling();
                    let Some(info) = mctx.fns.get(&name) else {
                        return Err(type_error(
                            format!("`img.check_layout` argument `{name}` is not a resolvable fn"),
                            call_span,
                        ));
                    };
                    if !is_layout_assert_fn(&info.ast) {
                        return Err(type_error(
                            format!(
                                "`img.check_layout` argument `{name}` must carry `@layout_assert`"
                            ),
                            call_span,
                        ));
                    }
                    check_layout_assert_fn(&info.ast, &info.decl, mctx)?;
                }
                _ => {
                    return Err(type_error(
                        "`img.check_layout` takes a bare `@layout_assert` fn name".to_string(),
                        call_span,
                    ));
                }
            }
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "Image.check_layout".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
                    args: vec![("f".to_string(), f)],
                },
            })
        }
        "seal" => {
            if !args.is_empty() {
                return Err(type_error(
                    "`img.seal` takes no arguments".to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                span: call_span,
                ty: image_type(),
                kind: TypedExprKind::Intrinsic {
                    key: "Image.seal".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
                    args: vec![],
                },
            })
        }
        _ => Err(type_error(
            format!("`Image` has no builder method `{name}`"),
            call_span,
        )),
    }
}

pub(crate) fn check_image_decl_method_intrinsic(
    receiver: TypedExpr,
    name: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if name != "handle" {
        return Err(type_error(
            format!("`ImageDecl` has no method `{name}`"),
            fspan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            "`decl.handle` takes no arguments".to_string(),
            call_span,
        ));
    }
    let _ = (fctx, mctx);
    let handle_ty = match &receiver.ty {
        Type::Named(marker, targs) if marker == "ImageDecl" && targs.len() == 1 => {
            let TypeArg::Type(inner) = &targs[0] else {
                unreachable!("ImageDecl's argument is a type")
            };
            Type::Named("Actor".to_string(), vec![TypeArg::Type(inner.clone())])
        }
        _ => image_decl_type(),
    };
    Ok(TypedExpr {
        span: call_span,
        ty: handle_ty,
        kind: TypedExprKind::Intrinsic {
            key: "ImageDecl.handle".to_string(),
            receiver: Some(Box::new(receiver)),
            type_arg: None,
            const_arg: None,
            args: vec![],
        },
    })
}

pub(crate) fn check_struct_construction(
    local_name: &str,
    s: &StructInfo,
    targs: &[TypeArg],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let self_ty = Type::Named(local_name.to_string(), targs.to_vec());
    if contains_opaque_field(&self_ty, mctx)
        && !(is_opaque_field_name(local_name, mctx) && mctx.module_path == "field")
    {
        return Err(type_error(
            "P004: opaque `Field` may not be stored in a user struct value".to_string(),
            call_span,
        ));
    }
    if let Some((ia, id)) = s.init() {
        let typed_args = check_call_args(&ia.params, &id.params, args, call_span, fctx, mctx)?;
        let key = if targs.is_empty() {
            CalleeKey::Method(local_name.to_string(), "init".to_string())
        } else {
            CalleeKey::MethodInstance(
                generics::canonical_key(InstKind::Struct, local_name, targs),
                "init".to_string(),
            )
        };
        let ret_ty = match &id.ret {
            Type::Unit => self_ty.clone(),
            Type::Result(ok, err) if **ok == Type::Unit => {
                Type::Result(Box::new(self_ty.clone()), err.clone())
            }
            _ => {
                return Err(unimplemented_at(
                    "a non-standard init return type is",
                    call_span,
                ));
            }
        };
        return Ok(TypedExpr {
            span: call_span,
            ty: ret_ty,
            kind: TypedExprKind::Call {
                callee: key,
                receiver: None,
                args: typed_args,
            },
        });
    }
    let fields = check_struct_literal(local_name, s, args, call_span, fctx, mctx)?;
    Ok(TypedExpr {
        span: call_span,
        ty: self_ty,
        kind: TypedExprKind::StructLiteral {
            name: local_name.to_string(),
            fields,
        },
    })
}

fn wrap_struct_field_mode(mode: AccessMode, value: TypedExpr, span: Span) -> TypedExpr {
    match mode {
        AccessMode::Take => TypedExpr {
            span,
            ty: value.ty.clone(),
            kind: TypedExprKind::Take(Box::new(value)),
        },
        AccessMode::Read | AccessMode::Mut => value,
    }
}

pub(crate) fn check_struct_literal(
    local_name: &str,
    s: &StructInfo,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<(String, TypedExpr)>, SemaError> {
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
        check_field_privacy(local_name, &fields[0].0, s, args[0].span, mctx)?;
        let vt = check_expr(&args[0].value, Some(&fields[0].1), fctx, mctx)?;
        let vt = wrap_struct_field_mode(args[0].mode, vt, args[0].span);
        return Ok(vec![(fields[0].0.clone(), vt)]);
    }
    let mut bound = vec![false; fields.len()];
    let mut slots: Vec<Option<TypedExpr>> = (0..fields.len()).map(|_| None).collect();
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
        check_field_privacy(local_name, label, s, a.span, mctx)?;
        let fty = fields[idx].1.clone();
        let vt = check_expr(&a.value, Some(&fty), fctx, mctx)?;
        let vt = wrap_struct_field_mode(a.mode, vt, a.span);
        slots[idx] = Some(vt);
    }
    for (i, (name, _, has_default)) in fields.iter().enumerate() {
        if !bound[i] && !has_default {
            return Err(type_error(format!("missing field `{name}`"), call_span));
        }
    }
    let out = fields
        .iter()
        .zip(slots.into_iter())
        .filter_map(|((name, _, _), v)| v.map(|vt| (name.clone(), vt)))
        .collect();
    Ok(out)
}

pub(crate) fn check_call_args(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
    let mut bound = vec![false; decl_params.len()];
    let mut slots: Vec<TypedCallArg> = (0..decl_params.len())
        .map(|_| TypedCallArg {
            mode: AccessMode::Read,
            value: None,
        })
        .collect();
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
        let pname = decl_params[idx].name.as_str();
        let use_kind = match pname {
            "length" | "len" => Some("a length"),
            "capacity" | "size" => Some("an allocation size"),
            _ => None,
        };
        if let Some(kind) = use_kind {
            let probe = check_expr(&a.value, None, fctx, mctx)?;
            if is_untrusted_type(&probe.ty) {
                return Err(untrusted_use_error(kind, a.span));
            }
        }
        let vt = check_expr(&a.value, Some(&pty), fctx, mctx)?;
        slots[idx] = TypedCallArg {
            mode: a.mode,
            value: Some(vt),
        };
    }
    for (i, p) in decl_params.iter().enumerate() {
        if !bound[i] && ast_params[i].default.is_none() {
            return Err(type_error(
                format!("missing argument for parameter `{}`", p.name),
                call_span,
            ));
        }
    }
    Ok(slots)
}

pub(crate) fn check_positional_args(
    params: &[(AccessMode, Type)],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
    if args.len() != params.len() {
        return Err(arity_error(params.len(), args.len(), call_span));
    }
    let mut out = Vec::with_capacity(args.len());
    for (a, (_mode, ty)) in args.iter().zip(params.iter()) {
        if a.label.is_some() {
            return Err(type_error(
                "labeled arguments require a named function".to_string(),
                a.span,
            ));
        }
        out.push(TypedCallArg {
            mode: a.mode,
            value: Some(check_expr(&a.value, Some(ty), fctx, mctx)?),
        });
    }
    Ok(out)
}

pub(crate) fn resolve_enum_for_variant_construction<'a>(
    enum_name: &str,
    info: &'a EnumInfo,
    expected: Option<&Type>,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<(Vec<types::TypeArg>, std::borrow::Cow<'a, types::DeclEnum>), SemaError> {
    if info.generics.is_empty() {
        return Ok((vec![], std::borrow::Cow::Borrowed(&info.decl)));
    }
    match expected {
        Some(Type::Named(n, args)) if n == enum_name => {
            let decl = generics::instantiate_enum(mctx, enum_name, args, span)?;
            Ok((args.clone(), std::borrow::Cow::Owned(decl)))
        }
        Some(other) => Err(type_error(
            format!(
                "expected `{}`, found a `{enum_name}` variant",
                types::render_type(other)
            ),
            span,
        )),
        None => Err(type_error(
            format!("cannot infer type arguments for `{enum_name}` variant construction"),
            span,
        )),
    }
}

pub(crate) fn check_variant_args(
    payload: &[Type],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
    if args.len() != payload.len() {
        return Err(arity_error(payload.len(), args.len(), call_span));
    }
    let mut out = Vec::with_capacity(args.len());
    for (a, ty) in args.iter().zip(payload.iter()) {
        out.push(TypedCallArg {
            mode: a.mode,
            value: Some(check_expr(&a.value, Some(ty), fctx, mctx)?),
        });
    }
    Ok(out)
}

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
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => None,
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
        Expr::ArrayRepeat(_, elem, count) => {
            scan_expr_forbidden(elem).or_else(|| scan_expr_forbidden(count))
        }
        _ => None,
    }
}

fn scan_closure_await(body: &ClosureBody) -> Option<Span> {
    match body {
        ClosureBody::Expr(e) => scan_expr_await(e),
        ClosureBody::Suite(stmts) => stmts.iter().find_map(scan_stmt_await),
    }
}

fn scan_stmt_await(s: &Stmt) -> Option<Span> {
    match s {
        Stmt::Assign(a) => scan_expr_await(&a.target).or_else(|| scan_expr_await(&a.value)),
        Stmt::If(i) => scan_expr_await(&i.cond)
            .or_else(|| i.then_branch.iter().find_map(scan_stmt_await))
            .or_else(|| {
                i.elifs.iter().find_map(|e| {
                    scan_expr_await(&e.cond).or_else(|| e.body.iter().find_map(scan_stmt_await))
                })
            })
            .or_else(|| {
                i.else_branch
                    .as_ref()
                    .and_then(|b| b.iter().find_map(scan_stmt_await))
            }),
        Stmt::Match(m) => scan_expr_await(&m.scrutinee).or_else(|| {
            m.arms.iter().find_map(|a| {
                a.guard
                    .as_ref()
                    .and_then(scan_expr_await)
                    .or_else(|| a.body.iter().find_map(scan_stmt_await))
            })
        }),
        Stmt::For(f) => {
            scan_expr_await(&f.iterable).or_else(|| f.body.iter().find_map(scan_stmt_await))
        }
        Stmt::While(w) => {
            scan_expr_await(&w.cond).or_else(|| w.body.iter().find_map(scan_stmt_await))
        }
        Stmt::Return(_, e) => e.as_ref().and_then(scan_expr_await),
        Stmt::Assert(a) => {
            scan_expr_await(&a.cond).or_else(|| a.message.as_ref().and_then(scan_expr_await))
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => scan_expr_await(e),
            DeferBody::Suite(s) => s.iter().find_map(scan_stmt_await),
        },
        Stmt::With(w) => {
            scan_expr_await(&w.expr).or_else(|| w.body.iter().find_map(scan_stmt_await))
        }
        Stmt::Send(_, e) | Stmt::Expr(_, e) => scan_expr_await(e),
        Stmt::ComptimeIf(c) => scan_expr_await(&c.cond)
            .or_else(|| c.then_branch.iter().find_map(scan_stmt_await))
            .or_else(|| {
                c.else_branch
                    .as_ref()
                    .and_then(|b| b.iter().find_map(scan_stmt_await))
            }),
        Stmt::ComptimeAssert(_, e, m) => {
            scan_expr_await(e).or_else(|| m.as_ref().and_then(scan_expr_await))
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => None,
    }
}

fn scan_expr_await(e: &Expr) -> Option<Span> {
    match e {
        Expr::Unary(span, UnaryOp::Await, _) => Some(*span),
        Expr::Unary(_, _, inner)
        | Expr::Try(_, inner)
        | Expr::Not(_, inner)
        | Expr::Send(_, inner) => scan_expr_await(inner),
        Expr::Field(base, _, _) => scan_expr_await(base),
        Expr::Index(base, _, args) => {
            scan_expr_await(base).or_else(|| args.iter().find_map(scan_expr_await))
        }
        Expr::Call(callee, _, args) => {
            scan_expr_await(callee).or_else(|| args.iter().find_map(|a| scan_expr_await(&a.value)))
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_expr_await(l).or_else(|| scan_expr_await(r))
        }
        Expr::Range(_, a, b, _) => scan_expr_await(a).or_else(|| scan_expr_await(b)),
        Expr::Is(_, s, _) => scan_expr_await(s),
        Expr::DotVariant(_, _, args) => args.iter().find_map(|a| scan_expr_await(&a.value)),
        Expr::Closure(c) => scan_closure_await(&c.body),
        Expr::Tuple(_, items) | Expr::List(_, items) => items.iter().find_map(scan_expr_await),
        Expr::ArrayRepeat(_, elem, count) => {
            scan_expr_await(elem).or_else(|| scan_expr_await(count))
        }
        _ => None,
    }
}

pub(crate) fn type_error(message: String, span: Span) -> SemaError {
    SemaError::at("type", message, span)
}

pub(crate) fn missing_method_error(
    message: String,
    type_name: &str,
    method_name: &str,
    span: Span,
) -> SemaError {
    let mut e = type_error(message, span);
    e.missing_method = Some((type_name.to_string(), method_name.to_string()));
    e
}

pub(crate) fn arity_error(expected: usize, found: usize, span: Span) -> SemaError {
    type_error(
        format!("expected {expected} argument(s), found {found}"),
        span,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_int_range_boundaries() {
        let span = Span::default();
        let cases: Vec<(&str, Type, i128, bool)> = vec![
            ("u8 min", Type::U8, 0, true),
            ("u8 max", Type::U8, 255, true),
            ("u8 above max", Type::U8, 256, false),
            ("u8 below min", Type::U8, -1, false),
            ("i8 min", Type::I8, -128, true),
            ("i8 max", Type::I8, 127, true),
            ("i8 above max", Type::I8, 128, false),
            ("i8 below min", Type::I8, -129, false),
            ("u64 min", Type::U64, 0, true),
            ("u64 max", Type::U64, u64::MAX as i128, true),
            ("u64 below min", Type::U64, -1, false),
            ("u64 above max", Type::U64, u64::MAX as i128 + 1, false),
            (
                "usize behaves like u64",
                Type::Usize,
                u64::MAX as i128,
                true,
            ),
            ("i64 min", Type::I64, i64::MIN as i128, true),
            ("i64 max", Type::I64, i64::MAX as i128, true),
            ("i64 above max", Type::I64, i64::MAX as i128 + 1, false),
            ("i64 below min", Type::I64, i64::MIN as i128 - 1, false),
            (
                "isize behaves like i64",
                Type::Isize,
                i64::MIN as i128,
                true,
            ),
        ];
        for (msg, ty, value, expect_ok) in cases {
            let result = check_int_range(value, &ty, span);
            assert_eq!(result.is_ok(), expect_ok, "{msg}: value {value}");
        }
    }

    #[test]
    fn synth_int_literal_unconstrained_defaulting() {
        let span = Span::default();
        let small = synth_int_literal(span, "100", None).expect("fits i64");
        assert!(
            matches!(small.ty, Type::I64),
            "a small unconstrained literal defaults to i64, found {:?}",
            small.ty
        );

        let only_u64 = (i64::MAX as i128 + 1).to_string();
        let ty = synth_int_literal(span, &only_u64, None).expect("fits u64 only");
        assert!(
            matches!(ty.ty, Type::U64),
            "a literal beyond i64::MAX but within u64::MAX defaults to u64, found {:?}",
            ty.ty
        );

        let too_big = (u64::MAX as i128 + 1).to_string();
        assert!(
            synth_int_literal(span, &too_big, None).is_err(),
            "a literal beyond u64::MAX has no default type"
        );
    }

    #[test]
    fn synth_int_literal_expected_type_cases() {
        let span = Span::default();
        assert!(synth_int_literal(span, "255", Some(&Type::U8)).is_ok());
        assert!(synth_int_literal(span, "256", Some(&Type::U8)).is_err());
        assert!(
            synth_int_literal(span, "0", Some(&Type::Bool)).is_err(),
            "an integer literal cannot check against a non-integer expected type"
        );
    }

    #[test]
    fn synth_float_literal_cases() {
        let span = Span::default();
        assert!(matches!(
            synth_float_literal(span, "1.0", Some(&Type::F32)),
            Ok(TypedExpr {
                span: _,
                ty: Type::F32,
                ..
            })
        ));
        assert!(matches!(
            synth_float_literal(span, "1.0", Some(&Type::F64)),
            Ok(TypedExpr {
                span: _,
                ty: Type::F64,
                ..
            })
        ));
        assert!(
            synth_float_literal(span, "1.0", Some(&Type::U8)).is_err(),
            "a float literal cannot check against a non-float expected type"
        );
        assert!(matches!(
            synth_float_literal(span, "1.0", None),
            Ok(TypedExpr {
                span: _,
                ty: Type::F64,
                ..
            })
        ));
    }

    #[test]
    fn types_eq_is_span_insensitive() {
        let len_a = Expr::Int(
            Span {
                line: 1,
                col: 1,
                ..Default::default()
            },
            "3".to_string(),
        );
        let len_b = Expr::Int(
            Span {
                line: 42,
                col: 7,
                ..Default::default()
            },
            "3".to_string(),
        );
        let a = Type::Array(Box::new(Type::U8), Box::new(len_a.clone()));
        let b = Type::Array(Box::new(Type::U8), Box::new(len_b.clone()));
        assert!(
            types_eq(&a, &b),
            "[u8; 3] at two different spans must compare equal under types_eq"
        );
        assert_ne!(
            len_a, len_b,
            "the two length exprs differ by span under derived PartialEq"
        );

        let len_c = Expr::Int(
            Span {
                line: 1,
                col: 1,
                ..Default::default()
            },
            "4".to_string(),
        );
        let c = Type::Array(Box::new(Type::U8), Box::new(len_c));
        assert!(
            !types_eq(&a, &c),
            "[u8; 3] and [u8; 4] must not compare equal"
        );

        let named_a = Type::Named("Ring".to_string(), vec![types::TypeArg::Const(len_a)]);
        let named_b = Type::Named("Ring".to_string(), vec![types::TypeArg::Const(len_b)]);
        assert!(
            types_eq(&named_a, &named_b),
            "Ring[3] at two different spans must compare equal under types_eq"
        );

        let shared_a = Type::Named(
            "DmaShared".to_string(),
            vec![
                types::TypeArg::Pool("BlockControl".to_string()),
                types::TypeArg::Type(Type::Named("RingControl".to_string(), vec![])),
            ],
        );
        let shared_b = Type::Named(
            "DmaShared".to_string(),
            vec![
                types::TypeArg::Pool("BlockControl".to_string()),
                types::TypeArg::Type(Type::Named("RingControl".to_string(), vec![])),
            ],
        );
        assert!(
            types_eq(&shared_a, &shared_b),
            "DmaShared[P, L] with equal Pool args must compare equal"
        );
        let shared_other = Type::Named(
            "DmaShared".to_string(),
            vec![
                types::TypeArg::Pool("OtherPool".to_string()),
                types::TypeArg::Type(Type::Named("RingControl".to_string(), vec![])),
            ],
        );
        assert!(
            !types_eq(&shared_a, &shared_other),
            "DmaShared with distinct Pool args must not compare equal"
        );
    }

    #[test]
    fn entropy_intrinsic_types_bytes_n_with_const_arg() {
        let src = "module examples.entropy_sema

pub fn sample() -> Bytes[8]:
    return entropy[8]()
";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let prog = crate::sema::check_typed(&module, "test.wr").expect("check");
        let f = prog.fns.get("sample").expect("sample");
        let TypedStmtKind::Return(Some(e)) = &f.body.last().expect("ret").kind else {
            panic!("expected return");
        };
        assert!(
            matches!(&e.ty, Type::Bytes(Some(len)) if literal_array_len(len) == Some(8)),
            "expected Bytes[8], got {:?}",
            e.ty
        );
        match &e.kind {
            TypedExprKind::Intrinsic {
                key,
                const_arg,
                args,
                ..
            } => {
                assert_eq!(key, "entropy");
                assert_eq!(*const_arg, Some(8));
                assert!(args.is_empty());
            }
            other => panic!("expected Intrinsic, got {other:?}"),
        }
    }

    #[test]
    fn entropy_rejects_zero_and_over_max() {
        for (n, _) in [(0u64, "zero"), (65u64, "over max")] {
            let src = format!(
                "module examples.entropy_bad_{n}

pub fn sample() -> Bytes[{n}]:
    return entropy[{n}]()
"
            );
            let tokens = crate::syntax::lexer::lex(&src).expect("lex");
            let module = crate::syntax::parser::parse(tokens).expect("parse");
            let err = crate::sema::check_typed(&module, "test.wr").expect_err("must reject");
            assert!(
                err.message.contains("1..=") || err.message.contains("length"),
                "n={n}: {}",
                err.message
            );
        }
    }
}
