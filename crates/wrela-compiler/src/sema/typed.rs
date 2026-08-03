use std::collections::{BTreeMap, BTreeSet};

use crate::sema::types::{self, Type};
use crate::syntax::ast::{AccessMode, BinOp, Span};

pub type EffectMap = BTreeMap<(String, String), AccessMode>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeKey {
    Fn(String),
    FnInstance(String),
    Method(String, String),
    MethodInstance(String, String),
}

impl CalleeKey {
    pub fn spelling(&self) -> String {
        match self {
            CalleeKey::Fn(name) => name.clone(),
            CalleeKey::FnInstance(key) => key.clone(),
            CalleeKey::Method(ty, member) => format!("{ty}.{member}"),
            CalleeKey::MethodInstance(key, member) => format!("{key}.{member}"),
        }
    }
}

pub fn is_restricted_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "Image"
            | "Image.device"
            | "Image.driver"
            | "Image.actor"
            | "Image.pool"
            | "Image.dma_pool"
            | "Image.renderer"
            | "Image.on_failure"
            | "Image.check_layout"
            | "Image.seal"
            | "ImageDecl.handle"
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedCallArg {
    pub mode: AccessMode,
    pub value: Option<TypedExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExpr {
    pub ty: Type,
    pub span: Span,
    pub kind: TypedExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    Int(String),
    Float(String),
    Str(String),
    BStr(String),
    Char(String),
    Bool(bool),
    Unit,
    Local(String),
    Const(String),
    Static(String),
    FnRef(CalleeKey),
    Field(Box<TypedExpr>, String),
    Index(Box<TypedExpr>, Box<TypedExpr>),
    Call {
        callee: CalleeKey,
        receiver: Option<Box<TypedExpr>>,
        args: Vec<TypedCallArg>,
    },
    CallValue(Box<TypedExpr>, Vec<TypedCallArg>),
    ToScalar(Box<TypedExpr>),
    Neg(Box<TypedExpr>),
    BitNot(Box<TypedExpr>),
    Take(Box<TypedExpr>),
    Try(Box<TypedExpr>, Option<CalleeKey>),
    Binary(BinOp, Box<TypedExpr>, Box<TypedExpr>),
    OpCall(CalleeKey, Box<TypedExpr>, Box<TypedExpr>),
    Is(Box<TypedExpr>, Box<TypedPattern>),
    Not(Box<TypedExpr>),
    And(Box<TypedExpr>, Box<TypedExpr>),
    Or(Box<TypedExpr>, Box<TypedExpr>),
    EnumConstruct {
        enum_name: String,
        variant: String,
        args: Vec<TypedCallArg>,
    },
    Closure {
        params: Vec<TypedClosureParam>,
        body: TypedClosureBody,
    },
    Tuple(Vec<TypedExpr>),
    List(Vec<TypedExpr>),
    StructLiteral {
        name: String,
        fields: Vec<(String, TypedExpr)>,
    },
    Panic(Box<TypedExpr>),
    Intrinsic {
        key: String,
        receiver: Option<Box<TypedExpr>>,
        type_arg: Option<Type>,
        const_arg: Option<u64>,
        args: Vec<(String, TypedExpr)>,
    },
    PoolName(String),
    Await(Box<TypedExpr>),
    Send(Box<TypedExpr>),
    GroupChild(CalleeKey),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedClosureParam {
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedClosureBody {
    Expr(Box<TypedExpr>),
    Suite(Vec<TypedStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedPattern {
    pub ty: Type,
    pub span: Span,
    pub kind: TypedPatternKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedPatternKind {
    Wildcard,
    Literal(Box<TypedExpr>),
    Binding(String),
    Take(Box<TypedPattern>),
    Variant {
        enum_name: String,
        variant: String,
        payload: Vec<TypedPattern>,
    },
    Tuple(Vec<TypedPattern>),
    Array(Vec<TypedPattern>),
    Or(Vec<TypedPattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedStmt {
    pub span: Span,
    pub kind: TypedStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedElif {
    pub cond: TypedExpr,
    pub body: Vec<TypedStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedMatchArm {
    pub pattern: TypedPattern,
    pub guard: Option<TypedExpr>,
    pub body: Vec<TypedStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedForIter {
    Range(TypedExpr, TypedExpr, bool),
    Expr(TypedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedDeferBody {
    Expr(Box<TypedExpr>),
    Suite(Vec<TypedStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStmtKind {
    Let {
        name: String,
        ty: Type,
        value: TypedExpr,
    },
    Assign {
        target: TypedExpr,
        value: TypedExpr,
    },
    If {
        cond: TypedExpr,
        then_branch: Vec<TypedStmt>,
        elifs: Vec<TypedElif>,
        else_branch: Option<Vec<TypedStmt>>,
    },
    Match {
        scrutinee: TypedExpr,
        arms: Vec<TypedMatchArm>,
    },
    For {
        name: String,
        elem_ty: Type,
        take_binding: bool,
        iter: TypedForIter,
        body: Vec<TypedStmt>,
        budget: Option<u64>,
    },
    While {
        cond: TypedExpr,
        body: Vec<TypedStmt>,
        budget: Option<u64>,
    },
    Break,
    Continue,
    Pass,
    Return(Option<TypedExpr>),
    Assert {
        cond: TypedExpr,
        message: Option<TypedExpr>,
    },
    ComptimeAssert {
        span: Span,
        cond: TypedExpr,
        message: Option<TypedExpr>,
    },
    Defer(TypedDeferBody),
    ExprStmt(TypedExpr),
    BareSend {
        span: Span,
        expr: TypedExpr,
    },
    WithGroup {
        capacity: Option<TypedExpr>,
        deadline: Option<TypedExpr>,
        as_name: Option<String>,
        body: Vec<TypedStmt>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedParam {
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
    pub default: Option<TypedExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedFn {
    pub receiver: Option<(AccessMode, Type)>,
    pub params: Vec<TypedParam>,
    pub ret: Type,
    pub body: Vec<TypedStmt>,
    pub is_async: bool,
    pub is_task: bool,
    pub is_layout_assert: bool,
    pub is_pub: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedConst {
    pub ty: Type,
    pub value: TypedExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedStatic {
    pub ty: Type,
    pub addr: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypedStruct {
    pub name: String,
    pub fields: Vec<String>,
    pub field_types: BTreeMap<String, Type>,
    /// Pixels contracts keyed by declaration-order field index. Source names
    /// remain presentation-only; renames cannot perturb the typed contract
    /// identity consumed by later renderer stages.
    pub field_contracts: BTreeMap<Vec<usize>, crate::sema::attrs::FieldContracts>,
    pub field_defaults: BTreeMap<String, TypedExpr>,
    pub methods: BTreeMap<String, TypedFn>,
    pub assoc_fns: BTreeMap<String, TypedFn>,
    pub init: Option<TypedFn>,
    pub is_resource: bool,
    pub is_actor: bool,
    pub is_driver: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedEnum {
    pub variants: Vec<String>,
    pub variant_payload_types: Vec<Vec<Type>>,
    /// Generic type parameter names aligned with declaration type/const
    /// arguments. Const-generic positions are `None`.
    pub generic_type_params: Vec<Option<String>>,
    pub methods: BTreeMap<String, TypedFn>,
    pub assoc_fns: BTreeMap<String, TypedFn>,
}

impl TypedEnum {
    pub fn from_variants(variants: Vec<String>) -> Self {
        let variant_payload_types = variants.iter().map(|_| Vec::new()).collect();
        Self {
            variants,
            variant_payload_types,
            generic_type_params: Vec::new(),
            methods: BTreeMap::new(),
            assoc_fns: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedInstantiation {
    Fn(TypedFn),
    Struct(TypedStruct),
    Enum(TypedEnum),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    Comptime,
    Runtime,
    Exhaustive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDecl {
    pub name: String,
    pub kind: TestKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnboundedSyncLoop {
    pub fn_name: String,
    pub span: crate::syntax::ast::Span,
    pub ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PixelsFnKind {
    Field,
    Material,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelsFnMeta {
    pub kind: PixelsFnKind,
    pub params_type: Option<Type>,
    pub material_type: Option<Type>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypedProgram {
    pub consts: BTreeMap<String, TypedConst>,
    pub statics: BTreeMap<String, TypedStatic>,
    pub fns: BTreeMap<String, TypedFn>,
    pub structs: BTreeMap<String, TypedStruct>,
    pub tests: Vec<TestDecl>,
    pub enums: BTreeMap<String, TypedEnum>,
    pub instantiations: BTreeMap<String, TypedInstantiation>,
    pub image_fn: Option<String>,
    pub declared_pools: BTreeSet<String>,
    pub layouts: Vec<types::LayoutType>,
    pub blk_capacity_sectors: Option<u64>,
    pub virtqueue_configures: Vec<(String, u16)>,
    pub reserve_permit_demands: Vec<crate::syntax::ast::Span>,
    pub unbounded_sync_loops: Vec<UnboundedSyncLoop>,
    pub effects: EffectMap,
    pub imported: ImportedDecls,
    pub pixels_fns: BTreeMap<String, PixelsFnMeta>,
    /// Canonical declaring module for every function name visible while this
    /// module was checked. This includes generic functions, which do not
    /// otherwise have an entry in `fns`.
    pub fn_decl_modules: BTreeMap<String, String>,
    /// Canonical declaration name paired with `fn_decl_modules`; imported
    /// aliases retain their target declaration identity here.
    pub fn_decl_names: BTreeMap<String, String>,
    /// Canonical declaring module for every nominal struct or enum name
    /// visible while this module was checked.
    pub type_decl_modules: BTreeMap<String, String>,
    /// Canonical declaration name paired with `type_decl_modules`; imported
    /// aliases retain their target declaration identity here.
    pub type_decl_names: BTreeMap<String, String>,
    /// Canonical module path for stable typed metadata keys.
    pub module_path: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedDecls {
    pub consts: BTreeMap<String, TypedConst>,
    pub fns: BTreeMap<String, TypedFn>,
    pub structs: BTreeMap<String, TypedStruct>,
    pub enums: BTreeMap<String, TypedEnum>,
    pub instantiations: BTreeMap<String, TypedInstantiation>,
    pub unresolvable: BTreeMap<String, String>,
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn ty(t: &Type) -> String {
    types::render_type(t)
}

pub fn dump(program: &TypedProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (name, meta) in &program.pixels_fns {
        let kind = match meta.kind {
            PixelsFnKind::Field => "field",
            PixelsFnKind::Material => "material",
        };
        let params = meta
            .params_type
            .as_ref()
            .map(ty)
            .unwrap_or_else(|| "none".to_string());
        let material = meta
            .material_type
            .as_ref()
            .map(ty)
            .unwrap_or_else(|| "none".to_string());
        let key = if program.module_path.is_empty() {
            name.clone()
        } else {
            format!("{}::{name}", program.module_path)
        };
        push_line(
            &mut out,
            1,
            &format!(
                "PixelsFn key={key} kind={kind} input_index=0 params_index={} \
                 params={params} material={material}",
                meta.params_type.as_ref().map(|_| "1").unwrap_or("none")
            ),
        );
    }
    for (name, c) in &program.consts {
        push_line(&mut out, 1, &format!("Const name={name} ty={}", ty(&c.ty)));
        dump_expr(&c.value, 2, &mut out);
    }
    for (name, f) in &program.fns {
        push_line(&mut out, 1, &format!("Fn name={name} ret={}", ty(&f.ret)));
        dump_fn_body(f, 2, &mut out);
    }
    for (name, s) in &program.structs {
        push_line(&mut out, 1, &format!("Struct name={name}"));
        dump_struct_body(s, 2, &mut out);
    }
    for (key, inst) in &program.instantiations {
        push_line(&mut out, 1, &format!("Instantiation key={key}"));
        match inst {
            TypedInstantiation::Fn(f) => {
                push_line(&mut out, 2, &format!("Fn ret={}", ty(&f.ret)));
                dump_fn_body(f, 3, &mut out);
            }
            TypedInstantiation::Struct(s) => {
                push_line(&mut out, 2, "Struct");
                dump_struct_body(s, 3, &mut out);
            }
            TypedInstantiation::Enum(_) => push_line(&mut out, 2, "Enum"),
        }
    }
    out
}

fn dump_param(p: &TypedParam, depth: usize, out: &mut String) {
    push_line(
        out,
        depth,
        &format!(
            "Param name={} mode={} ty={}",
            p.name,
            p.mode.as_str(),
            ty(&p.ty)
        ),
    );
    if let Some(def) = &p.default {
        push_line(out, depth + 1, "Default");
        dump_expr(def, depth + 2, out);
    }
}

fn dump_fn_body(f: &TypedFn, depth: usize, out: &mut String) {
    if let Some((mode, self_ty)) = &f.receiver {
        push_line(
            out,
            depth,
            &format!("Receiver mode={} ty={}", mode.as_str(), ty(self_ty)),
        );
    }
    for p in &f.params {
        dump_param(p, depth, out);
    }
    push_line(out, depth, "Body");
    dump_stmts(&f.body, depth + 1, out);
}

fn dump_struct_body(s: &TypedStruct, depth: usize, out: &mut String) {
    for index in 0..s.fields.len() {
        let Some(contracts) = s.field_contracts.get([index].as_slice()) else {
            continue;
        };
        if contracts.range.is_none() && contracts.rate.is_none() {
            continue;
        }
        let mut line = format!("PixelsParamField path=[{index}]");
        if let Some(range) = contracts.range {
            let endpoints = range
                .exact_integer
                .map(|(min, max)| format!("{min},{max}"))
                .unwrap_or_else(|| format!("{},{}", range.min, range.max));
            line.push_str(&format!(
                " range=[{endpoints}] exact_integer={}",
                range.integer
            ));
        }
        if let Some(rate) = contracts.rate {
            line.push_str(&format!(
                " rate=[{},{}]",
                rate.max_delta, rate.max_second_delta
            ));
        }
        push_line(out, depth, &line);
    }
    for (name, def) in &s.field_defaults {
        push_line(out, depth, &format!("FieldDefault name={name}"));
        dump_expr(def, depth + 1, out);
    }
    for (name, f) in &s.methods {
        push_line(
            out,
            depth,
            &format!("Method name={name} ret={}", ty(&f.ret)),
        );
        dump_fn_body(f, depth + 1, out);
    }
    for (name, f) in &s.assoc_fns {
        push_line(
            out,
            depth,
            &format!("AssocFn name={name} ret={}", ty(&f.ret)),
        );
        dump_fn_body(f, depth + 1, out);
    }
    if let Some(f) = &s.init {
        push_line(out, depth, &format!("Init ret={}", ty(&f.ret)));
        dump_fn_body(f, depth + 1, out);
    }
}

fn dump_stmts(stmts: &[TypedStmt], depth: usize, out: &mut String) {
    for s in stmts {
        dump_stmt(s, depth, out);
    }
}

fn dump_stmt(stmt: &TypedStmt, depth: usize, out: &mut String) {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty: t, value } => {
            push_line(out, depth, &format!("Let name={name} ty={}", ty(t)));
            dump_expr(value, depth + 1, out);
        }
        TypedStmtKind::Assign { target, value } => {
            push_line(out, depth, "Assign");
            dump_expr(target, depth + 1, out);
            dump_expr(value, depth + 1, out);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            push_line(out, depth, "If");
            dump_expr(cond, depth + 1, out);
            push_line(out, depth + 1, "Then");
            dump_stmts(then_branch, depth + 2, out);
            for elif in elifs {
                push_line(out, depth + 1, "Elif");
                dump_expr(&elif.cond, depth + 2, out);
                dump_stmts(&elif.body, depth + 2, out);
            }
            if let Some(b) = else_branch {
                push_line(out, depth + 1, "Else");
                dump_stmts(b, depth + 2, out);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            push_line(out, depth, "Match");
            dump_expr(scrutinee, depth + 1, out);
            for arm in arms {
                push_line(out, depth + 1, "Case");
                dump_pattern(&arm.pattern, depth + 2, out);
                if let Some(g) = &arm.guard {
                    push_line(out, depth + 2, "Guard");
                    dump_expr(g, depth + 3, out);
                }
                dump_stmts(&arm.body, depth + 2, out);
            }
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            take_binding,
            iter,
            body,
            budget,
        } => {
            let mut header = format!("For name={name} elem_ty={}", ty(elem_ty));
            if *take_binding {
                header.push_str(" take=true");
            }
            if let Some(n) = budget {
                header.push_str(&format!(" budget={n}"));
            }
            push_line(out, depth, &header);
            match iter {
                TypedForIter::Range(from, to, incl) => {
                    push_line(out, depth + 1, &format!("Range inclusive={incl}"));
                    dump_expr(from, depth + 2, out);
                    dump_expr(to, depth + 2, out);
                }
                TypedForIter::Expr(e) => dump_expr(e, depth + 1, out),
            }
            push_line(out, depth + 1, "Body");
            dump_stmts(body, depth + 2, out);
        }
        TypedStmtKind::While { cond, body, budget } => {
            let header = match budget {
                Some(n) => format!("While budget={n}"),
                None => "While".to_string(),
            };
            push_line(out, depth, &header);
            dump_expr(cond, depth + 1, out);
            push_line(out, depth + 1, "Body");
            dump_stmts(body, depth + 2, out);
        }
        TypedStmtKind::Break => push_line(out, depth, "Break"),
        TypedStmtKind::Continue => push_line(out, depth, "Continue"),
        TypedStmtKind::Pass => push_line(out, depth, "Pass"),
        TypedStmtKind::Return(value) => {
            push_line(out, depth, "Return");
            if let Some(v) = value {
                dump_expr(v, depth + 1, out);
            }
        }
        TypedStmtKind::Assert { cond, message } => {
            push_line(out, depth, "Assert");
            dump_expr(cond, depth + 1, out);
            if let Some(m) = message {
                dump_expr(m, depth + 1, out);
            }
        }
        TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            push_line(out, depth, "ComptimeAssert");
            dump_expr(cond, depth + 1, out);
            if let Some(m) = message {
                dump_expr(m, depth + 1, out);
            }
        }
        TypedStmtKind::Defer(body) => {
            push_line(out, depth, "Defer");
            match body {
                TypedDeferBody::Expr(e) => dump_expr(e, depth + 1, out),
                TypedDeferBody::Suite(stmts) => {
                    push_line(out, depth + 1, "Body");
                    dump_stmts(stmts, depth + 2, out);
                }
            }
        }
        TypedStmtKind::ExprStmt(e) => dump_expr(e, depth, out),
        TypedStmtKind::BareSend { expr, .. } => {
            push_line(out, depth, "BareSend");
            dump_expr(expr, depth + 1, out);
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => {
            let mut header = "WithGroup".to_string();
            if let Some(name) = as_name {
                header.push_str(&format!(" as={name}"));
            }
            push_line(out, depth, &header);
            if let Some(c) = capacity {
                push_line(out, depth + 1, "Capacity");
                dump_expr(c, depth + 2, out);
            }
            if let Some(d) = deadline {
                push_line(out, depth + 1, "Deadline");
                dump_expr(d, depth + 2, out);
            }
            push_line(out, depth + 1, "Body");
            dump_stmts(body, depth + 2, out);
        }
    }
}

fn dump_pattern(p: &TypedPattern, depth: usize, out: &mut String) {
    match &p.kind {
        TypedPatternKind::Wildcard => push_line(out, depth, &format!("Wildcard ty={}", ty(&p.ty))),
        TypedPatternKind::Literal(e) => {
            push_line(out, depth, &format!("PatternLiteral ty={}", ty(&p.ty)));
            dump_expr(e.as_ref(), depth + 1, out);
        }
        TypedPatternKind::Binding(name) => {
            push_line(out, depth, &format!("Binding name={name} ty={}", ty(&p.ty)))
        }
        TypedPatternKind::Take(inner) => {
            push_line(out, depth, &format!("TakePattern ty={}", ty(&p.ty)));
            dump_pattern(inner, depth + 1, out);
        }
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } => {
            push_line(
                out,
                depth,
                &format!(
                    "VariantPattern enum={enum_name} variant={variant} ty={}",
                    ty(&p.ty)
                ),
            );
            for pat in payload {
                dump_pattern(pat, depth + 1, out);
            }
        }
        TypedPatternKind::Tuple(elems) => {
            push_line(out, depth, &format!("TuplePattern ty={}", ty(&p.ty)));
            for e in elems {
                dump_pattern(e, depth + 1, out);
            }
        }
        TypedPatternKind::Array(elems) => {
            push_line(out, depth, &format!("ArrayPattern ty={}", ty(&p.ty)));
            for e in elems {
                dump_pattern(e, depth + 1, out);
            }
        }
        TypedPatternKind::Or(alts) => {
            push_line(out, depth, &format!("OrPattern ty={}", ty(&p.ty)));
            for a in alts {
                dump_pattern(a, depth + 1, out);
            }
        }
    }
}

fn dump_call_args(args: &[TypedCallArg], ty_for_default: &Type, depth: usize, out: &mut String) {
    for a in args {
        match &a.value {
            Some(e) => {
                if a.mode != AccessMode::Read {
                    push_line(out, depth, &format!("Arg mode={}", a.mode.as_str()));
                    dump_expr(e, depth + 1, out);
                } else {
                    dump_expr(e, depth, out);
                }
            }
            None => push_line(out, depth, &format!("DefaultArg ty={}", ty(ty_for_default))),
        }
    }
}

fn dump_expr(e: &TypedExpr, depth: usize, out: &mut String) {
    let t = ty(&e.ty);
    match &e.kind {
        TypedExprKind::Int(text) => push_line(out, depth, &format!("Int text={text} ty={t}")),
        TypedExprKind::Float(text) => push_line(out, depth, &format!("Float text={text} ty={t}")),
        TypedExprKind::Str(text) => push_line(out, depth, &format!("Str text={text} ty={t}")),
        TypedExprKind::BStr(text) => push_line(out, depth, &format!("BStr text={text} ty={t}")),
        TypedExprKind::Char(text) => push_line(out, depth, &format!("Char text={text} ty={t}")),
        TypedExprKind::Bool(v) => push_line(out, depth, &format!("Bool value={v} ty={t}")),
        TypedExprKind::Unit => push_line(out, depth, &format!("Unit ty={t}")),
        TypedExprKind::Local(name) => push_line(out, depth, &format!("Local name={name} ty={t}")),
        TypedExprKind::Const(name) => push_line(out, depth, &format!("Const name={name} ty={t}")),
        TypedExprKind::Static(name) => push_line(out, depth, &format!("Static name={name} ty={t}")),
        TypedExprKind::FnRef(key) => {
            push_line(out, depth, &format!("FnRef key={} ty={t}", key.spelling()))
        }
        TypedExprKind::Field(base, name) => {
            push_line(out, depth, &format!("Field name={name} ty={t}"));
            dump_expr(base, depth + 1, out);
        }
        TypedExprKind::Index(base, idx) => {
            push_line(out, depth, &format!("Index ty={t}"));
            dump_expr(base, depth + 1, out);
            dump_expr(idx, depth + 1, out);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            push_line(
                out,
                depth,
                &format!("Call key={} ty={t}", callee.spelling()),
            );
            if let Some(r) = receiver {
                push_line(out, depth + 1, "Receiver");
                dump_expr(r, depth + 2, out);
            }
            dump_call_args(args, &Type::Unit, depth + 1, out);
        }
        TypedExprKind::CallValue(callee, args) => {
            push_line(out, depth, &format!("CallValue ty={t}"));
            push_line(out, depth + 1, "Callee");
            dump_expr(callee, depth + 2, out);
            dump_call_args(args, &Type::Unit, depth + 1, out);
        }
        TypedExprKind::ToScalar(inner) => {
            push_line(out, depth, &format!("ToScalar ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Neg(inner) => {
            push_line(out, depth, &format!("Neg ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::BitNot(inner) => {
            push_line(out, depth, &format!("BitNot ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Take(inner) => {
            push_line(out, depth, &format!("Take ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Try(inner, conv) => {
            let mut header = format!("Try ty={t}");
            if let Some(key) = conv {
                header.push_str(&format!(" conv={}", key.spelling()));
            }
            push_line(out, depth, &header);
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Binary(op, l, r) => {
            push_line(out, depth, &format!("Binary op={} ty={t}", op.as_str()));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::OpCall(key, l, r) => {
            push_line(out, depth, &format!("OpCall key={} ty={t}", key.spelling()));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::Is(inner, pat) => {
            push_line(out, depth, &format!("Is ty={t}"));
            dump_expr(inner, depth + 1, out);
            dump_pattern(pat.as_ref(), depth + 1, out);
        }
        TypedExprKind::Not(inner) => {
            push_line(out, depth, &format!("Not ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::And(l, r) => {
            push_line(out, depth, &format!("And ty={t}"));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::Or(l, r) => {
            push_line(out, depth, &format!("Or ty={t}"));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::EnumConstruct {
            enum_name,
            variant,
            args,
        } => {
            push_line(
                out,
                depth,
                &format!("EnumConstruct enum={enum_name} variant={variant} ty={t}"),
            );
            dump_call_args(args, &Type::Unit, depth + 1, out);
        }
        TypedExprKind::Closure { params, body } => {
            push_line(out, depth, &format!("Closure ty={t}"));
            for p in params {
                push_line(
                    out,
                    depth + 1,
                    &format!(
                        "ClosureParam name={} mode={} ty={}",
                        p.name,
                        p.mode.as_str(),
                        ty(&p.ty)
                    ),
                );
            }
            match body {
                TypedClosureBody::Expr(e) => dump_expr(e, depth + 1, out),
                TypedClosureBody::Suite(stmts) => {
                    push_line(out, depth + 1, "Body");
                    dump_stmts(stmts, depth + 2, out);
                }
            }
        }
        TypedExprKind::Tuple(items) => {
            push_line(out, depth, &format!("Tuple ty={t}"));
            for i in items {
                dump_expr(i, depth + 1, out);
            }
        }
        TypedExprKind::List(items) => {
            push_line(out, depth, &format!("List ty={t}"));
            for i in items {
                dump_expr(i, depth + 1, out);
            }
        }
        TypedExprKind::StructLiteral { name, fields } => {
            push_line(out, depth, &format!("StructLiteral name={name} ty={t}"));
            for (fname, fval) in fields {
                push_line(out, depth + 1, &format!("Field name={fname}"));
                dump_expr(fval, depth + 2, out);
            }
        }
        TypedExprKind::Panic(msg) => {
            push_line(out, depth, &format!("Panic ty={t}"));
            dump_expr(msg, depth + 1, out);
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            const_arg,
            args,
        } => {
            let mut header = format!("Intrinsic key={key}");
            if let Some(ta) = type_arg {
                header.push_str(&format!(" type_arg={}", ty(ta)));
            }
            if let Some(n) = const_arg {
                header.push_str(&format!(" const_arg={n}"));
            }
            header.push_str(&format!(" ty={t}"));
            push_line(out, depth, &header);
            if let Some(r) = receiver {
                push_line(out, depth + 1, "Receiver");
                dump_expr(r, depth + 2, out);
            }
            for (label, val) in args {
                push_line(out, depth + 1, &format!("Arg label={label}"));
                dump_expr(val, depth + 2, out);
            }
        }
        TypedExprKind::PoolName(name) => {
            push_line(out, depth, &format!("PoolName name={name} ty={t}"));
        }
        TypedExprKind::Await(inner) => {
            push_line(out, depth, &format!("Await ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Send(inner) => {
            push_line(out, depth, &format!("Send ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::GroupChild(key) => {
            push_line(
                out,
                depth,
                &format!("GroupChild key={} ty={t}", key.spelling()),
            );
        }
    }
}

pub(crate) fn rekey_struct_names(s: &mut TypedStruct, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    if let Some(to) = subs.get(&s.name) {
        s.name = to.clone();
    }
    for ty in s.field_types.values_mut() {
        rekey_type(ty, subs);
    }
    for e in s.field_defaults.values_mut() {
        rekey_expr(e, subs);
    }
    for f in s.methods.values_mut() {
        rekey_fn(f, subs);
    }
    for f in s.assoc_fns.values_mut() {
        rekey_fn(f, subs);
    }
    if let Some(f) = s.init.as_mut() {
        rekey_fn(f, subs);
    }
}

pub(crate) fn rekey_enum_names(e: &mut TypedEnum, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    for payloads in &mut e.variant_payload_types {
        for ty in payloads {
            rekey_type(ty, subs);
        }
    }
    for f in e.methods.values_mut() {
        rekey_fn(f, subs);
    }
    for f in e.assoc_fns.values_mut() {
        rekey_fn(f, subs);
    }
}

pub(crate) fn rekey_fn_names(f: &mut TypedFn, subs: &BTreeMap<String, String>) {
    rekey_fn(f, subs);
}

pub(crate) fn rekey_const_names(c: &mut TypedConst, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    rekey_type(&mut c.ty, subs);
    rekey_expr(&mut c.value, subs);
}

pub(crate) fn collect_named_types_from_struct(s: &TypedStruct, out: &mut BTreeSet<String>) {
    out.insert(s.name.clone());
    for ty in s.field_types.values() {
        types::collect_named_type_names(ty, out);
    }
    for f in s.methods.values() {
        collect_named_types_from_fn(f, out);
    }
    for f in s.assoc_fns.values() {
        collect_named_types_from_fn(f, out);
    }
    if let Some(f) = &s.init {
        collect_named_types_from_fn(f, out);
    }
}

pub(crate) fn collect_named_types_from_enum(e: &TypedEnum, out: &mut BTreeSet<String>) {
    for payloads in &e.variant_payload_types {
        for ty in payloads {
            types::collect_named_type_names(ty, out);
        }
    }
    for f in e.methods.values() {
        collect_named_types_from_fn(f, out);
    }
    for f in e.assoc_fns.values() {
        collect_named_types_from_fn(f, out);
    }
}

pub(crate) fn collect_named_types_from_fn(f: &TypedFn, out: &mut BTreeSet<String>) {
    if let Some((_, ty)) = &f.receiver {
        types::collect_named_type_names(ty, out);
    }
    for p in &f.params {
        types::collect_named_type_names(&p.ty, out);
    }
    types::collect_named_type_names(&f.ret, out);
}

fn rekey_fn(f: &mut TypedFn, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    if let Some((_, ty)) = f.receiver.as_mut() {
        rekey_type(ty, subs);
    }
    for p in &mut f.params {
        rekey_type(&mut p.ty, subs);
        if let Some(d) = p.default.as_mut() {
            rekey_expr(d, subs);
        }
    }
    rekey_type(&mut f.ret, subs);
    for st in &mut f.body {
        rekey_stmt(st, subs);
    }
}

fn rekey_type(ty: &mut Type, subs: &BTreeMap<String, String>) {
    match ty {
        Type::Array(elem, _) => rekey_type(elem, subs),
        Type::Tuple(elems) => {
            for e in elems {
                rekey_type(e, subs);
            }
        }
        Type::Option(inner) => rekey_type(inner, subs),
        Type::Result(ok, err) => {
            rekey_type(ok, subs);
            rekey_type(err, subs);
        }
        Type::Own(_, inner) | Type::Static(inner) => rekey_type(inner, subs),
        Type::Fn(params, ret) => {
            for (_, p) in params {
                rekey_type(p, subs);
            }
            rekey_type(ret, subs);
        }
        Type::Named(name, targs) => {
            if let Some(to) = subs.get(name) {
                *name = to.clone();
            }
            for a in targs {
                rekey_type_arg(a, subs);
            }
        }
        Type::Bytes(_)
        | Type::String(_)
        | Type::Bool
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
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::Unit
        | Type::Never
        | Type::Str
        | Type::Generic(_) => {}
    }
}

fn rekey_type_arg(a: &mut types::TypeArg, subs: &BTreeMap<String, String>) {
    match a {
        types::TypeArg::Type(t) => rekey_type(t, subs),
        types::TypeArg::Const(_) | types::TypeArg::Bound(_) | types::TypeArg::Pool(_) => {}
    }
}

fn rekey_callee(key: &mut CalleeKey, subs: &BTreeMap<String, String>) {
    match key {
        CalleeKey::Method(sname, _) => {
            if let Some(to) = subs.get(sname) {
                *sname = to.clone();
            }
        }
        CalleeKey::FnInstance(ikey) | CalleeKey::MethodInstance(ikey, _) => {
            *ikey = rekey_canonical_key(ikey, subs);
        }
        CalleeKey::Fn(_) => {}
    }
}

pub(crate) fn rekey_canonical_key(key: &str, subs: &BTreeMap<String, String>) -> String {
    if subs.is_empty() {
        return key.to_string();
    }
    let mut out = String::with_capacity(key.len());
    let bytes = key.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &key[start..i];
            if let Some(to) = subs.get(ident) {
                out.push_str(to);
            } else {
                out.push_str(ident);
            }
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    out
}

pub(crate) fn rekey_instantiation(inst: &mut TypedInstantiation, subs: &BTreeMap<String, String>) {
    match inst {
        TypedInstantiation::Fn(f) => rekey_fn_names(f, subs),
        TypedInstantiation::Struct(s) => rekey_struct_names(s, subs),
        TypedInstantiation::Enum(enumeration) => {
            for payloads in &mut enumeration.variant_payload_types {
                for payload in payloads {
                    rekey_type(payload, subs);
                }
            }
            for function in enumeration
                .methods
                .values_mut()
                .chain(enumeration.assoc_fns.values_mut())
            {
                rekey_fn_names(function, subs);
            }
        }
    }
}

fn rekey_expr(e: &mut TypedExpr, subs: &BTreeMap<String, String>) {
    rekey_type(&mut e.ty, subs);
    match &mut e.kind {
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
        TypedExprKind::FnRef(key) | TypedExprKind::GroupChild(key) => rekey_callee(key, subs),
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Panic(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base) => rekey_expr(base, subs),
        TypedExprKind::Index(base, idx) => {
            rekey_expr(base, subs);
            rekey_expr(idx, subs);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            rekey_callee(callee, subs);
            if let Some(r) = receiver {
                rekey_expr(r, subs);
            }
            for a in args {
                if let Some(v) = a.value.as_mut() {
                    rekey_expr(v, subs);
                }
            }
        }
        TypedExprKind::CallValue(f, args) => {
            rekey_expr(f, subs);
            for a in args {
                if let Some(v) = a.value.as_mut() {
                    rekey_expr(v, subs);
                }
            }
        }
        TypedExprKind::Try(inner, conv) => {
            rekey_expr(inner, subs);
            if let Some(key) = conv {
                rekey_callee(key, subs);
            }
        }
        TypedExprKind::Binary(_, l, r) | TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            rekey_expr(l, subs);
            rekey_expr(r, subs);
        }
        TypedExprKind::OpCall(key, l, r) => {
            rekey_callee(key, subs);
            rekey_expr(l, subs);
            rekey_expr(r, subs);
        }
        TypedExprKind::Is(inner, pat) => {
            rekey_expr(inner, subs);
            rekey_pattern(pat, subs);
        }
        TypedExprKind::EnumConstruct {
            enum_name, args, ..
        } => {
            if let Some(to) = subs.get(enum_name) {
                *enum_name = to.clone();
            }
            for a in args {
                if let Some(v) = a.value.as_mut() {
                    rekey_expr(v, subs);
                }
            }
        }
        TypedExprKind::Closure { params, body } => {
            for p in params {
                rekey_type(&mut p.ty, subs);
            }
            match body {
                TypedClosureBody::Expr(e) => rekey_expr(e, subs),
                TypedClosureBody::Suite(stmts) => {
                    for st in stmts {
                        rekey_stmt(st, subs);
                    }
                }
            }
        }
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                rekey_expr(i, subs);
            }
        }
        TypedExprKind::StructLiteral { name, fields } => {
            if let Some(to) = subs.get(name) {
                *name = to.clone();
            }
            for (_, f) in fields {
                rekey_expr(f, subs);
            }
        }
        TypedExprKind::Intrinsic {
            receiver,
            type_arg,
            args,
            ..
        } => {
            if let Some(r) = receiver {
                rekey_expr(r, subs);
            }
            if let Some(t) = type_arg {
                rekey_type(t, subs);
            }
            for (_, a) in args {
                rekey_expr(a, subs);
            }
        }
    }
}

fn rekey_pattern(p: &mut TypedPattern, subs: &BTreeMap<String, String>) {
    rekey_type(&mut p.ty, subs);
    match &mut p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => rekey_expr(e, subs),
        TypedPatternKind::Take(inner) => rekey_pattern(inner, subs),
        TypedPatternKind::Variant {
            enum_name, payload, ..
        } => {
            if let Some(to) = subs.get(enum_name) {
                *enum_name = to.clone();
            }
            for sp in payload {
                rekey_pattern(sp, subs);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for sp in items {
                rekey_pattern(sp, subs);
            }
        }
        TypedPatternKind::Or(alts) => {
            for a in alts {
                rekey_pattern(a, subs);
            }
        }
    }
}

fn rekey_stmt(st: &mut TypedStmt, subs: &BTreeMap<String, String>) {
    match &mut st.kind {
        TypedStmtKind::Let { ty, value, .. } => {
            rekey_type(ty, subs);
            rekey_expr(value, subs);
        }
        TypedStmtKind::Assign { target, value } => {
            rekey_expr(target, subs);
            rekey_expr(value, subs);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            rekey_expr(cond, subs);
            for s in then_branch {
                rekey_stmt(s, subs);
            }
            for e in elifs {
                rekey_expr(&mut e.cond, subs);
                for s in &mut e.body {
                    rekey_stmt(s, subs);
                }
            }
            if let Some(body) = else_branch {
                for s in body {
                    rekey_stmt(s, subs);
                }
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            rekey_expr(scrutinee, subs);
            for arm in arms {
                rekey_pattern(&mut arm.pattern, subs);
                if let Some(g) = arm.guard.as_mut() {
                    rekey_expr(g, subs);
                }
                for s in &mut arm.body {
                    rekey_stmt(s, subs);
                }
            }
        }
        TypedStmtKind::For {
            elem_ty,
            iter,
            body,
            ..
        } => {
            rekey_type(elem_ty, subs);
            match iter {
                TypedForIter::Range(a, b, _) => {
                    rekey_expr(a, subs);
                    rekey_expr(b, subs);
                }
                TypedForIter::Expr(e) => rekey_expr(e, subs),
            }
            for s in body {
                rekey_stmt(s, subs);
            }
        }
        TypedStmtKind::While { cond, body, .. } => {
            rekey_expr(cond, subs);
            for s in body {
                rekey_stmt(s, subs);
            }
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(v) => {
            if let Some(e) = v {
                rekey_expr(e, subs);
            }
        }
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            rekey_expr(cond, subs);
            if let Some(m) = message {
                rekey_expr(m, subs);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => rekey_expr(e, subs),
            TypedDeferBody::Suite(stmts) => {
                for s in stmts {
                    rekey_stmt(s, subs);
                }
            }
        },
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => {
            rekey_expr(e, subs);
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                rekey_expr(c, subs);
            }
            if let Some(d) = deadline {
                rekey_expr(d, subs);
            }
            for s in body {
                rekey_stmt(s, subs);
            }
        }
    }
}
