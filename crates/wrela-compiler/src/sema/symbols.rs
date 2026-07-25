//! Collect + resolve (plans/M2.md decision 3, passes 1-2; item A).
//!
//! `collect` builds the module item table: every top-level `const`/`fn`/
//! `struct`/`enum`/`pool` name must be unique (02-language.md §2, §7 —
//! declarations share one module-scope namespace); a second declaration of
//! the same name is an error at the second occurrence.
//!
//! `resolve` then walks every body and checks that every identifier binds
//! to the prelude (prelude.rs), a module declaration, or a local
//! introduced by assignment/parameter/pattern binding/for binding/closure
//! param (02-language.md §3.2): unknown names and shadowed outer locals
//! are errors. Type names in annotations resolve through the exact same
//! lookup. Two things deliberately do **not** resolve yet, because they
//! need type information item B provides: a member/method name after
//! `.` (the base's type decides whether it is a field or a method), and a
//! leading-dot enum variant reference (`.Found`, `enum_name: None`) whose
//! enum is inferred from the expected type. `Enum.Name(...)` (an explicit
//! enum name) is an ordinary identifier and does resolve.
//!
//! `comptime if`/`comptime assert` and `from ... import` bodies are not
//! expanded/checked here: imports fail closed (decision 7, handled by
//! `resolve`'s first check, below); comptime evaluation is item C's job
//! (02-language.md §12) — a `comptime if` item/member/statement is simply
//! not visited by either pass in item A.

use std::collections::BTreeMap;

use crate::sema::SemaError;
use crate::sema::imports::ImportBindings;
use crate::syntax::ast::*;

/// The module item table: declared name -> declaring span. `BTreeMap`
/// throughout sema, per ROADMAP's determinism doctrine, even though only
/// exact-name lookups are ever done against it — the ordering guarantee
/// is free and keeps any future iteration stable too.
pub type SymbolTable = BTreeMap<String, Span>;

/// Pass 1: the module item table (02-language.md §2, §7). `comptime if`
/// items are skipped — their branches are not yet expanded (see the
/// module doc comment above) — so they contribute no names here.
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
        // 03-hardware.md §1 (plans/M7.md item A): "Their constructors are
        // not source-visible: no address, import, or cast creates one."
        // Declaring a struct under a capability's own name is the one
        // route that would hand a source author a real, constructible
        // value spelled `DeviceCap(...)` — every other prelude name is
        // merely shadowed by such a declaration, harmlessly, but these
        // four are the names an *unforgeable* rule is about. Rejected
        // here, at the one pass that already owns "what names does this
        // module declare".
        if crate::eval::image_checks::is_sealed_authority_type_name(name) {
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
        Item::Fn(f) => Some((f.name.as_str(), f.span)),
        Item::Struct(s) => Some((s.name.as_str(), s.span)),
        Item::Enum(e) => Some((e.name.as_str(), e.span)),
        Item::Pool(p) => Some((p.name.as_str(), p.span)),
        Item::ComptimeIf(_) => None,
    }
}

/// Pass 2: name resolution (plans/M4.md item A, decision 2: whole-
/// program import resolution replaces the old "imports fail closed"
/// stub — `imports` is this module's own already-resolved
/// `sema::imports::ImportBindings`, empty for every single-module
/// caller that has no closure to resolve against (the plain `check`/
/// `check_typed` entry, the fuzzer, `specialize.rs`'s const skeleton),
/// so a module with no real imports resolves byte-identically to
/// before this item (existing goldens are not affected either way).
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
        Item::Fn(f) => resolve_fn(f, symtab, imports),
        Item::Struct(s) => resolve_struct(s, symtab, imports),
        Item::Enum(e) => resolve_enum(e, symtab, imports),
        Item::Pool(_) => Ok(()),
        Item::ComptimeIf(_) => Ok(()), // comptime evaluation is item C's job
    }
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

/// A struct's own generics are visible to every member (fields, methods,
/// `init`) — each member gets a fresh `Resolver` seeded with them, since
/// members share no locals with one another.
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
            Member::ComptimeIf(_) => {} // comptime evaluation is item C's job
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

/// One local name scope: a flat map for the current lexical level. A
/// closure (`ClosureExpr`) pushes a new one, and so does every
/// `if`/`elif`/`else` branch, `while`/`for` body, and `match` arm
/// (`resolve_stmts_scoped`) — a *typed* declaration (`name: T = value`)
/// made inside one of those is scoped to it alone and must not resolve
/// afterward (err-mwir-if-else-scope-leak: two sibling branches each
/// declaring their own `value: u64` were wrongly treated as the same
/// binding). A *plain*, untyped assignment (`name = value`) is the
/// arm-merge idiom instead (02-language.md §8.1) and is introduced via
/// `reassign_or_introduce`, which searches outward across these same
/// pushed scopes so a name conditionally set in every arm still resolves
/// once the construct closes.
/// Whether a name introduced on one control-flow path is *definitely*
/// bound on every other (the untyped case) is the flow pass's job (items
/// E/F: definite initialization — 02-language.md §3.2), not name
/// resolution's; a name only needs to exist in some scope still on the
/// stack to resolve here.
type Scope = BTreeMap<String, Span>;

/// One function-like unit's resolution state: the module item table
/// (read-only, shared), this module's own resolved import bindings
/// (plans/M4.md item A — empty outside a real closure), plus the scope
/// stack it is walking.
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

    /// Introduces `name` at `span` into the innermost scope: a plain
    /// reassignment if `name` is already there (02-language.md §3.2:
    /// "reassignment keeps the type"), a shadow error if any *outer*
    /// scope already binds it ("shadowing an outer local is an error"),
    /// otherwise a fresh binding.
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

    /// A plain (untyped) assignment target, `name = value` — the
    /// arm-merge idiom (02-language.md §8.1), never a shadow error: a
    /// name already bound in *any* scope still on the stack (an outer
    /// function-level local, or a sibling branch's own scope while that
    /// branch is still open — the two-arms-of-one-`if` case, not a
    /// *different*, already-closed sibling, since its scope has already
    /// been popped by the time this one runs) is reused as a
    /// reassignment; otherwise this is the first occurrence anywhere and
    /// introduces a fresh binding, scoped to wherever it textually is
    /// (the innermost open scope) — same as a typed declaration, since
    /// nothing merges a name that was never bound outside the one branch
    /// that first assigned it.
    fn reassign_or_introduce(&mut self, name: &str, span: Span) {
        if self.scopes.iter().any(|s| s.contains_key(name)) {
            return;
        }
        let last = self.scopes.len() - 1;
        self.scopes[last].insert(name.to_string(), span);
    }

    /// Runs `f` with a fresh innermost scope pushed for one `if`/`elif`/
    /// `else` branch, `while`/`for` body, or `match` arm — always popped
    /// again, even on error (`mod.rs::check_program`'s multi-module walk
    /// keeps going after one body errors).
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

    /// Resolves a read-position identifier — a local in the current or
    /// any enclosing scope, a module declaration, an imported name
    /// (plans/M4.md item A), or the prelude (02-language.md §3.2, §2).
    /// Also used for type names in annotations, per the item A brief
    /// (item B refines type checking; item A of M4 does not extend
    /// `types::declare`'s own type-name resolution to imports — an
    /// imported `struct`/`enum` name used *as a type annotation* still
    /// resolves here but fails at `declare` with the ordinary
    /// `error[type]: unknown type`, a real, narrow, reported scope gap,
    /// not a silent wrong answer).
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
        if super::prelude::is_builtin(name) {
            return Ok(());
        }
        Err(SemaError::at(
            "name",
            format!("unknown name `{name}`"),
            span,
        ))
    }

    /// A `[generics](receiver, params) -> ret` signature, shared by
    /// `fn`/`init` and, with a struct's/enum's own generics pre-seeded,
    /// by every member of that struct/enum (02-language.md §5.1, §7.1).
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
                // A single-segment pool name is a module-level `pool
                // Name` declaration and resolves like any identifier; a
                // dotted `Owner.Name` path names an actor-scoped pool
                // from outside it, which needs actor scoping (M6+) this
                // item does not have — left unresolved, like a
                // member-access name after `.`.
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
                    // `name: T = value` is a fresh declaration (an error
                    // if it would shadow an outer local); `name = value`
                    // is the arm-merge idiom's plain reassignment, which
                    // reuses a binding from any scope still open instead
                    // (02-language.md §3.2, §8.1 — see `Scope`'s own doc
                    // comment). Any other place (a field, an index) must
                    // already be bound, so it is resolved like any other
                    // read.
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
            // Each branch is its own scope, popped again once resolved,
            // so a typed declaration in `then_branch` cannot resolve in
            // an `elif`/`else` sibling or after the whole `if`
            // (err-mwir-if-else-scope-leak).
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
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => Ok(()),
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
            // comptime evaluation is item C's job (02-language.md §12).
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
                // `Enum.Name(...)` resolves its enum name like any other
                // identifier; a leading-dot `.Name(...)` (`enum_name:
                // None`) needs the scrutinee's type to know which enum,
                // so it is left unresolved (item B). The variant name
                // itself never resolves here either way — same reason.
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
            // plans/M9.md item D: interpolation interiors are ordinary
            // expressions (desugared to `.format()` + `String` concat).
            Expr::FStr(f) => {
                let desugared = crate::sema::fstring::desugar_fstring(f)?;
                self.resolve_expr(&desugared)
            }
            Expr::Name(span, name) => self.resolve_name(name, *span),
            // The name after `.` needs the base's type to know whether
            // it is a field or a method (item B); only the base resolves
            // here.
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
            // The variant name is inferred from the expected type (item
            // B); only the argument expressions resolve here.
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
