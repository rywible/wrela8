use std::collections::BTreeMap;

use crate::sema::SemaError;
use crate::sema::imports::ImportBindings;
use crate::syntax::ast::*;

pub type SymbolTable = BTreeMap<String, Span>;

pub fn collect(module: &Module) -> Result<SymbolTable, SemaError> {
    let mut table = SymbolTable::new();
    for item in &module.items {
        let Some((name, span)) = item_name(item) else {
            continue;
        };
        if table.contains_key(name) {
            return Err(SemaError::at(
                "name",
                format!("duplicate declaration `{name}`"),
                span,
            ));
        }
        if crate::sema::classes::name_holds_authority(name) {
            let kind = crate::eval::image_checks::sealed_authority_kind(name);
            return Err(SemaError::at(
                "name",
                format!(
                    "`{name}` is {kind} and cannot be declared: its constructor is not \
                     source-visible, and a declaration under its name would be one"
                ),
                span,
            ));
        }
        table.insert(name.to_string(), span);
    }
    Ok(table)
}

fn item_name(item: &Item) -> Option<(&str, Span)> {
    match item {
        Item::Const(c) => Some((c.name.as_str(), c.span)),
        Item::Static(s) => Some((s.name.as_str(), s.span)),
        Item::Fn(f) => Some((f.name.as_str(), f.span)),
        Item::Struct(s) => Some((s.name.as_str(), s.span)),
        Item::Enum(e) => Some((e.name.as_str(), e.span)),
        Item::Pool(p) => Some((p.name.as_str(), p.span)),
        Item::ComptimeIf(_) => None,
    }
}

pub fn resolve(
    module: &Module,
    symtab: &SymbolTable,
    imports: &ImportBindings,
) -> Result<(), SemaError> {
    for item in &module.items {
        resolve_item(item, symtab, imports)?;
    }
    Ok(())
}

fn resolve_item(
    item: &Item,
    symtab: &SymbolTable,
    imports: &ImportBindings,
) -> Result<(), SemaError> {
    match item {
        Item::Const(c) => resolve_const(c, symtab, imports),
        Item::Static(s) => resolve_static(s, symtab, imports),
        Item::Fn(f) => resolve_fn(f, symtab, imports),
        Item::Struct(s) => resolve_struct(s, symtab, imports),
        Item::Enum(e) => resolve_enum(e, symtab, imports),
        Item::Pool(_) => Ok(()),
        Item::ComptimeIf(_) => Ok(()),
    }
}

fn resolve_static(
    s: &crate::syntax::ast::StaticItem,
    symtab: &SymbolTable,
    imports: &ImportBindings,
) -> Result<(), SemaError> {
    let mut r = Resolver::new(symtab, imports);
    r.resolve_type(&s.ty)
}

fn resolve_const(
    c: &ConstItem,
    symtab: &SymbolTable,
    imports: &ImportBindings,
) -> Result<(), SemaError> {
    let mut r = Resolver::new(symtab, imports);
    if let Some(ty) = &c.ty {
        r.resolve_type(ty)?;
    }
    r.resolve_expr(&c.value)
}

fn resolve_fn(f: &FnItem, symtab: &SymbolTable, imports: &ImportBindings) -> Result<(), SemaError> {
    let mut r = Resolver::new(symtab, imports);
    r.resolve_signature(&f.generics, f.receiver.as_ref(), &f.params, f.ret.as_ref())?;
    if let Some(body) = &f.body {
        r.resolve_stmts(body)?;
    }
    Ok(())
}

fn resolve_struct(
    s: &StructItem,
    symtab: &SymbolTable,
    imports: &ImportBindings,
) -> Result<(), SemaError> {
    for m in &s.members {
        match m {
            Member::Field(field) => {
                let mut r = Resolver::new(symtab, imports);
                for g in &s.generics {
                    r.introduce_generic(g)?;
                }
                r.resolve_type(&field.ty)?;
                if let Some(d) = &field.default {
                    r.resolve_expr(d)?;
                }
            }
            Member::Fn(f) => {
                let mut r = Resolver::new(symtab, imports);
                for g in &s.generics {
                    r.introduce_generic(g)?;
                }
                r.resolve_signature(&f.generics, f.receiver.as_ref(), &f.params, f.ret.as_ref())?;
                if let Some(body) = &f.body {
                    r.resolve_stmts(body)?;
                }
            }
            Member::Init(i) => {
                let mut r = Resolver::new(symtab, imports);
                for g in &s.generics {
                    r.introduce_generic(g)?;
                }
                r.resolve_signature(&[], Some(&i.receiver), &i.params, i.ret.as_ref())?;
                r.resolve_stmts(&i.body)?;
            }
            Member::Pool(_) => {}
            Member::ComptimeIf(_) => {}
        }
    }
    Ok(())
}

fn resolve_enum(
    e: &EnumItem,
    symtab: &SymbolTable,
    imports: &ImportBindings,
) -> Result<(), SemaError> {
    for v in &e.variants {
        let mut r = Resolver::new(symtab, imports);
        for g in &e.generics {
            r.introduce_generic(g)?;
        }
        match &v.payload {
            VariantPayload::None => {}
            VariantPayload::Tuple(types) => {
                for t in types {
                    r.resolve_type(t)?;
                }
            }
            VariantPayload::Named(fields) => {
                for f in fields {
                    r.resolve_type(&f.ty)?;
                }
            }
        }
    }
    Ok(())
}

type Scope = BTreeMap<String, Span>;

struct Resolver<'a> {
    symtab: &'a SymbolTable,
    imports: &'a ImportBindings,
    scopes: Vec<Scope>,
}

impl<'a> Resolver<'a> {
    fn new(symtab: &'a SymbolTable, imports: &'a ImportBindings) -> Self {
        Resolver {
            symtab,
            imports,
            scopes: vec![Scope::new()],
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn introduce(&mut self, name: &str, span: Span) -> Result<(), SemaError> {
        let last = self.scopes.len() - 1;
        if self.scopes[last].contains_key(name) {
            return Ok(());
        }
        for scope in &self.scopes[..last] {
            if scope.contains_key(name) {
                return Err(SemaError::at(
                    "name",
                    format!("`{name}` shadows an outer local"),
                    span,
                ));
            }
        }
        self.scopes[last].insert(name.to_string(), span);
        Ok(())
    }

    fn reassign_or_introduce(&mut self, name: &str, span: Span) {
        if self.scopes.iter().any(|s| s.contains_key(name)) {
            return;
        }
        let last = self.scopes.len() - 1;
        self.scopes[last].insert(name.to_string(), span);
    }

    fn scoped(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), SemaError>,
    ) -> Result<(), SemaError> {
        self.push_scope();
        let result = f(self);
        self.pop_scope();
        result
    }

    fn introduce_generic(&mut self, g: &GenericParam) -> Result<(), SemaError> {
        match g {
            GenericParam::Type { span, name } => self.introduce(name, *span),
            GenericParam::Const { span, name, ty } => {
                self.resolve_type(ty)?;
                self.introduce(name, *span)
            }
        }
    }

    fn resolve_name(&self, name: &str, span: Span) -> Result<(), SemaError> {
        if self.scopes.iter().any(|s| s.contains_key(name)) {
            return Ok(());
        }
        if self.symtab.contains_key(name) {
            return Ok(());
        }
        if self.imports.contains_key(name) {
            return Ok(());
        }
        if is_resolvable_without_import(name) {
            return Ok(());
        }
        Err(SemaError::at(
            "name",
            format!("unknown name `{name}`"),
            span,
        ))
    }

    fn resolve_signature(
        &mut self,
        generics: &[GenericParam],
        receiver: Option<&Receiver>,
        params: &[Param],
        ret: Option<&Type>,
    ) -> Result<(), SemaError> {
        for g in generics {
            self.introduce_generic(g)?;
        }
        if let Some(r) = receiver {
            self.introduce("self", r.span)?;
        }
        for p in params {
            self.resolve_type(&p.ty)?;
            self.introduce(&p.name, p.span)?;
            if let Some(d) = &p.default {
                self.resolve_expr(d)?;
            }
        }
        if let Some(r) = ret {
            self.resolve_type(r)?;
        }
        Ok(())
    }

    fn resolve_type(&mut self, ty: &Type) -> Result<(), SemaError> {
        match ty {
            Type::Named(n) => {
                self.resolve_name(&n.name, n.span)?;
                for a in &n.args {
                    match a {
                        GenericArg::Type(t) => self.resolve_type(t)?,
                        GenericArg::Bound(e) | GenericArg::Expr(e) => self.resolve_expr(e)?,
                    }
                }
                Ok(())
            }
            Type::Array(a) => {
                self.resolve_type(&a.elem)?;
                self.resolve_expr(&a.len)
            }
            Type::Tuple(t) => {
                for e in &t.elems {
                    self.resolve_type(e)?;
                }
                Ok(())
            }
            Type::Own(o) => {
                if o.pool.len() == 1 {
                    self.resolve_name(&o.pool[0], o.span)?;
                }
                self.resolve_type(&o.inner)
            }
            Type::Fn(f) => {
                for p in &f.params {
                    self.resolve_type(&p.ty)?;
                }
                if let Some(r) = &f.ret {
                    self.resolve_type(r)?;
                }
                Ok(())
            }
        }
    }

    fn resolve_stmts(&mut self, stmts: &[Stmt]) -> Result<(), SemaError> {
        for s in stmts {
            self.resolve_stmt(s)?;
        }
        Ok(())
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) -> Result<(), SemaError> {
        match stmt {
            Stmt::Assign(a) => {
                self.resolve_expr(&a.value)?;
                if let Some(ty) = &a.ty {
                    self.resolve_type(ty)?;
                }
                match &a.target {
                    Expr::Name(span, name) => {
                        if a.ty.is_some() {
                            self.introduce(name, *span)
                        } else {
                            self.reassign_or_introduce(name, *span);
                            Ok(())
                        }
                    }
                    other => self.resolve_expr(other),
                }
            }
            Stmt::If(i) => {
                self.resolve_expr(&i.cond)?;
                self.scoped(|r| r.resolve_stmts(&i.then_branch))?;
                for elif in &i.elifs {
                    self.resolve_expr(&elif.cond)?;
                    self.scoped(|r| r.resolve_stmts(&elif.body))?;
                }
                if let Some(b) = &i.else_branch {
                    self.scoped(|r| r.resolve_stmts(b))?;
                }
                Ok(())
            }
            Stmt::Match(m) => {
                self.resolve_expr(&m.scrutinee)?;
                for arm in &m.arms {
                    self.scoped(|r| {
                        r.resolve_pattern(&arm.pattern)?;
                        if let Some(g) = &arm.guard {
                            r.resolve_expr(g)?;
                        }
                        r.resolve_stmts(&arm.body)
                    })?;
                }
                Ok(())
            }
            Stmt::For(f) => {
                self.resolve_expr(&f.iterable)?;
                self.scoped(|r| {
                    r.introduce(&f.name, f.span)?;
                    r.resolve_stmts(&f.body)
                })
            }
            Stmt::While(w) => {
                self.resolve_expr(&w.cond)?;
                self.scoped(|r| r.resolve_stmts(&w.body))
            }
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => Ok(()),
            Stmt::Return(_span, e) => match e {
                Some(e) => self.resolve_expr(e),
                None => Ok(()),
            },
            Stmt::Assert(a) => {
                self.resolve_expr(&a.cond)?;
                if let Some(m) = &a.message {
                    self.resolve_expr(m)?;
                }
                Ok(())
            }
            Stmt::Defer(d) => match &d.body {
                DeferBody::Expr(e) => self.resolve_expr(e),
                DeferBody::Suite(s) => self.resolve_stmts(s),
            },
            Stmt::With(w) => {
                self.resolve_expr(&w.expr)?;
                if let Some(name) = &w.as_name {
                    self.introduce(name, w.span)?;
                }
                self.resolve_stmts(&w.body)
            }
            Stmt::Send(_span, e) => self.resolve_expr(e),
            Stmt::Expr(_span, e) => self.resolve_expr(e),
            Stmt::ComptimeIf(_) | Stmt::ComptimeAssert(..) => Ok(()),
        }
    }

    fn resolve_pattern(&mut self, pattern: &Pattern) -> Result<(), SemaError> {
        match pattern {
            Pattern::Wildcard(_) => Ok(()),
            Pattern::Literal(_span, expr) => self.resolve_expr(expr),
            Pattern::Binding(span, name) => self.introduce(name, *span),
            Pattern::Take(_span, inner) => self.resolve_pattern(inner),
            Pattern::Variant {
                span,
                enum_name,
                variant: _,
                payload,
            } => {
                if let Some(name) = enum_name {
                    self.resolve_name(name, *span)?;
                }
                for p in payload {
                    self.resolve_pattern(p)?;
                }
                Ok(())
            }
            Pattern::Tuple(_span, items) | Pattern::Array(_span, items) => {
                for p in items {
                    self.resolve_pattern(p)?;
                }
                Ok(())
            }
            Pattern::Or(_span, alts) => {
                for p in alts {
                    self.resolve_pattern(p)?;
                }
                Ok(())
            }
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) -> Result<(), SemaError> {
        match expr {
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::BStr(..)
            | Expr::Char(..)
            | Expr::Bool(..)
            | Expr::Unit(..) => Ok(()),
            Expr::FStr(f) => {
                let desugared = crate::sema::fstring::desugar_fstring(f)?;
                self.resolve_expr(&desugared)
            }
            Expr::Name(span, name) => self.resolve_name(name, *span),
            Expr::Field(base, _span, _name) => self.resolve_expr(base),
            Expr::Index(base, _span, args) => {
                self.resolve_expr(base)?;
                for a in args {
                    self.resolve_expr(a)?;
                }
                Ok(())
            }
            Expr::Call(callee, _span, args) => {
                self.resolve_expr(callee)?;
                for a in args {
                    self.resolve_expr(&a.value)?;
                }
                Ok(())
            }
            Expr::Unary(_span, _op, inner) => self.resolve_expr(inner),
            Expr::Try(_span, inner) => self.resolve_expr(inner),
            Expr::Binary(_span, _op, l, r) => {
                self.resolve_expr(l)?;
                self.resolve_expr(r)
            }
            Expr::Range(_span, from, to, _incl) => {
                self.resolve_expr(from)?;
                self.resolve_expr(to)
            }
            Expr::Is(_span, scrutinee, pattern) => {
                self.resolve_expr(scrutinee)?;
                self.resolve_pattern(pattern)
            }
            Expr::Not(_span, inner) => self.resolve_expr(inner),
            Expr::And(_span, l, r) | Expr::Or(_span, l, r) => {
                self.resolve_expr(l)?;
                self.resolve_expr(r)
            }
            Expr::DotVariant(_span, _name, args) => {
                for a in args {
                    self.resolve_expr(&a.value)?;
                }
                Ok(())
            }
            Expr::Closure(c) => self.resolve_closure(c),
            Expr::Send(_span, inner) => self.resolve_expr(inner),
            Expr::Tuple(_span, items) | Expr::List(_span, items) => {
                for i in items {
                    self.resolve_expr(i)?;
                }
                Ok(())
            }
            Expr::ArrayRepeat(_span, elem, count) => {
                self.resolve_expr(elem)?;
                self.resolve_expr(count)
            }
        }
    }

    fn resolve_closure(&mut self, c: &ClosureExpr) -> Result<(), SemaError> {
        self.push_scope();
        let result = self.resolve_closure_inner(c);
        self.pop_scope();
        result
    }

    fn resolve_closure_inner(&mut self, c: &ClosureExpr) -> Result<(), SemaError> {
        for p in &c.params {
            if let Some(ty) = &p.ty {
                self.resolve_type(ty)?;
            }
            self.introduce(&p.name, p.span)?;
        }
        match &c.body {
            ClosureBody::Expr(e) => self.resolve_expr(e),
            ClosureBody::Suite(stmts) => self.resolve_stmts(stmts),
        }
    }
}

pub fn is_resolvable_without_import(name: &str) -> bool {
    super::prelude_scope::is_fixed_prelude_name(name)
        || super::types::is_builtin_type_name(name)
        || super::intrinsics::is_bare_resolvable(name)
        || super::prelude_scope::STDLIB_AUTO_VISIBLE.contains(&name)
        || super::prelude_scope::TIME_PRELUDE_NAMES.contains(&name)
}
