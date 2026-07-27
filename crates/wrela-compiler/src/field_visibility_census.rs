//! **Field-visibility violations census** (plans/M13.md item G1 / decision 7).
//!
//! 02-language.md §2: struct fields are module-private unless `pub`; only
//! the declaring module may construct, read, write, or pattern-bind a
//! non-`pub` field. G1 carries `DeclField.is_pub` and ships this census;
//! G3 flips the same sites to `error[sema]`. Until then the census is
//! **warn-only** — it never fails the build.
//!
//! Counts are re-measured live (do not trust the M9-OO 26/44/30/27
//! snapshot). Pattern-bind stays a column even though revision 0.1 has
//! no struct-field patterns — the normative sentence names the four
//! sites together, and G3 will refuse them uniformly when they exist.

use crate::sema::imports::ImportBindings;
use crate::sema::typed::{
    TypedClosureBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter, TypedInstantiation,
    TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::{DeclItem, DeclMember, Type};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One cross-module use of a non-`pub` field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub kind: Kind,
    /// Dotted module path of the use site.
    pub use_module: String,
    /// Dotted module path that declares the struct.
    pub decl_module: String,
    /// Local type spelling at the use site (`Duo` under an alias).
    pub struct_name: String,
    pub field: String,
    /// Exporter spelling when `struct_name` is an import alias.
    pub decl_struct: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Construct,
    Read,
    Write,
    Pattern,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Construct => "construct",
            Kind::Read => "read",
            Kind::Write => "write",
            Kind::Pattern => "pattern",
        }
    }
}

impl Violation {
    fn decl_struct_name(&self) -> &str {
        self.decl_struct.as_deref().unwrap_or(&self.struct_name)
    }
}

/// Census over one build closure (or an aggregation of several).
///
/// `violations` holds **one row per site** (a `Pair(a=…)` appearing in
/// three functions contributes three construct rows). `render` collapses
/// detail lines to unique `(kind, use, decl, struct, field)` for a
/// readable dump; `counts` uses the full site list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    pub violations: Vec<Violation>,
}

/// Count summary (also the corpus warn-only line).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub constructs: usize,
    pub reads: usize,
    pub writes: usize,
    pub patterns: usize,
    pub fields: usize,
}

impl Counts {
    pub fn render_line(&self) -> String {
        format!(
            "field-visibility census: constructs={} reads={} writes={} patterns={} fields_needing_pub={}",
            self.constructs, self.reads, self.writes, self.patterns, self.fields
        )
    }
}

impl Census {
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for v in &self.violations {
            match v.kind {
                Kind::Construct => c.constructs += 1,
                Kind::Read => c.reads += 1,
                Kind::Write => c.writes += 1,
                Kind::Pattern => c.patterns += 1,
            }
        }
        c.fields = self.fields_needing_pub().len();
        c
    }

    /// Distinct `(decl_module, decl_struct, field)` triples that would
    /// need `pub` (or a redesign) under enforcement.
    pub fn fields_needing_pub(&self) -> BTreeSet<(String, String, String)> {
        let mut out = BTreeSet::new();
        for v in &self.violations {
            out.insert((
                v.decl_module.clone(),
                v.decl_struct_name().to_string(),
                v.field.clone(),
            ));
        }
        out
    }

    /// Distinct dotted use-module paths that contributed a violation.
    pub fn packages_hit(&self) -> BTreeSet<String> {
        self.violations
            .iter()
            .map(|v| v.use_module.clone())
            .collect()
    }

    /// Stable dump / report text pinned by `golden/check-field-visibility-census`.
    /// Summary counts are per-site; detail lines are unique shapes.
    pub fn render(&self) -> String {
        let c = self.counts();
        let mut out = format!(
            "FieldVisibilityCensus constructs={} reads={} writes={} patterns={} fields={}\n",
            c.constructs, c.reads, c.writes, c.patterns, c.fields
        );
        let mut rows: Vec<Violation> = self.violations.iter().cloned().collect();
        rows.sort();
        rows.dedup();
        for v in rows {
            out.push_str(&format!(
                "  {} use={} decl={} struct={} field={}\n",
                v.kind.as_str(),
                v.use_module,
                v.decl_module,
                v.struct_name,
                v.field
            ));
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn extend(&mut self, other: Census) {
        self.violations.extend(other.violations);
    }
}

/// Build a census over one already-checked multi-module program.
///
/// `skip_modules` names auto-injected modules omitted from the
/// `--stage=check` dump (`core.time` / `core.runtime` when not explicitly
/// imported, and always-generated `core.__image_runtime`). Walking them
/// would flood the census with generated-table field traffic that is not
/// the G2 migration worklist.
pub fn census_programs(
    programs: &BTreeMap<Vec<String>, TypedProgram>,
    decl_items: &BTreeMap<Vec<String>, Vec<DeclItem>>,
    bindings: &BTreeMap<Vec<String>, ImportBindings>,
    skip_modules: &BTreeSet<Vec<String>>,
) -> Census {
    let mut field_pub: BTreeMap<(Vec<String>, String, String), bool> = BTreeMap::new();
    let mut declared_structs: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    for (mod_key, items) in decl_items {
        let mut names = BTreeSet::new();
        for item in items {
            if let DeclItem::Struct(s) = item {
                names.insert(s.name.clone());
                for m in &s.members {
                    if let DeclMember::Field(f) = m {
                        field_pub
                            .insert((mod_key.clone(), s.name.clone(), f.name.clone()), f.is_pub);
                    }
                }
            }
        }
        declared_structs.insert(mod_key.clone(), names);
    }

    let empty = ImportBindings::new();
    let mut violations = Vec::new();
    for (use_mod, program) in programs {
        if skip_modules.contains(use_mod) {
            continue;
        }
        let use_name = use_mod.join(".");
        let bs = bindings.get(use_mod).unwrap_or(&empty);
        let local_structs = declared_structs.get(use_mod);
        let mut ctx = WalkCtx {
            use_module: use_name,
            use_mod_key: use_mod.as_slice(),
            bindings: bs,
            local_structs,
            field_pub: &field_pub,
            declared_structs: &declared_structs,
            violations: &mut violations,
        };
        walk_program(program, &mut ctx);
    }
    // Keep every site for counts; sort only for determinism.
    violations.sort();
    Census { violations }
}

struct WalkCtx<'a> {
    use_module: String,
    use_mod_key: &'a [String],
    bindings: &'a ImportBindings,
    local_structs: Option<&'a BTreeSet<String>>,
    field_pub: &'a BTreeMap<(Vec<String>, String, String), bool>,
    declared_structs: &'a BTreeMap<Vec<String>, BTreeSet<String>>,
    violations: &'a mut Vec<Violation>,
}

impl WalkCtx<'_> {
    fn resolve_foreign_field(
        &self,
        type_name: &str,
        field: &str,
    ) -> Option<(String, String, String)> {
        if self.local_structs.is_some_and(|s| s.contains(type_name)) {
            return None;
        }
        let (decl_mod, decl_struct) = if let Some(b) = self.bindings.get(type_name) {
            (b.target_module.clone(), b.target_name.clone())
        } else {
            // HH-reachable: same spelling declared elsewhere.
            let mut found = None;
            for (mod_key, names) in self.declared_structs {
                if mod_key.as_slice() == self.use_mod_key {
                    continue;
                }
                if names.contains(type_name) {
                    found = Some((mod_key.clone(), type_name.to_string()));
                    break;
                }
            }
            found?
        };
        // Generated image-runtime tables are not the G2 migration worklist
        // (handwritten `core.runtime` indexes them by design).
        if decl_mod.as_slice() == crate::loader::IMAGE_RUNTIME_MODULE_KEY {
            return None;
        }
        let is_pub = self
            .field_pub
            .get(&(decl_mod.clone(), decl_struct.clone(), field.to_string()))
            .copied()
            .unwrap_or(true); // unknown field → not a visibility hit
        if is_pub {
            return None;
        }
        Some((decl_mod.join("."), decl_struct, field.to_string()))
    }

    fn record(&mut self, kind: Kind, type_name: &str, field: &str) {
        let Some((decl_module, decl_struct, field)) = self.resolve_foreign_field(type_name, field)
        else {
            return;
        };
        self.violations.push(Violation {
            kind,
            use_module: self.use_module.clone(),
            decl_module,
            struct_name: type_name.to_string(),
            field,
            decl_struct: Some(decl_struct),
        });
    }
}

fn walk_program(program: &TypedProgram, ctx: &mut WalkCtx<'_>) {
    for c in program.consts.values() {
        walk_expr(&c.value, ctx, Place::Value);
    }
    for f in program.fns.values() {
        walk_fn(f, ctx);
    }
    for s in program.structs.values() {
        walk_struct(s, ctx);
    }
    for e in program.enums.values() {
        for f in e.methods.values() {
            walk_fn(f, ctx);
        }
        for f in e.assoc_fns.values() {
            walk_fn(f, ctx);
        }
    }
    for inst in program.instantiations.values() {
        match inst {
            // Free-fn instantiations only. A Struct instantiation carries
            // the declaring module's method bodies under the importer's
            // program when the importer triggered the generic — those
            // `self.field` reads are same-module at the decl site and
            // must not count as cross-module violations here.
            TypedInstantiation::Fn(f) => walk_fn(f, ctx),
            TypedInstantiation::Struct(_) | TypedInstantiation::Enum => {}
        }
    }
}

fn walk_struct(s: &TypedStruct, ctx: &mut WalkCtx<'_>) {
    for def in s.field_defaults.values() {
        walk_expr(def, ctx, Place::Value);
    }
    for f in s.methods.values() {
        walk_fn(f, ctx);
    }
    for f in s.assoc_fns.values() {
        walk_fn(f, ctx);
    }
    if let Some(f) = &s.init {
        walk_fn(f, ctx);
    }
}

fn walk_fn(f: &TypedFn, ctx: &mut WalkCtx<'_>) {
    for p in &f.params {
        if let Some(d) = &p.default {
            walk_expr(d, ctx, Place::Value);
        }
    }
    for stmt in &f.body {
        walk_stmt(stmt, ctx);
    }
}

#[derive(Clone, Copy)]
enum Place {
    Value,
    AssignTarget,
}

fn walk_stmt(stmt: &TypedStmt, ctx: &mut WalkCtx<'_>) {
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => walk_expr(value, ctx, Place::Value),
        TypedStmtKind::Assign { target, value } => {
            walk_expr(target, ctx, Place::AssignTarget);
            walk_expr(value, ctx, Place::Value);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            walk_expr(cond, ctx, Place::Value);
            for s in then_branch {
                walk_stmt(s, ctx);
            }
            for e in elifs {
                walk_expr(&e.cond, ctx, Place::Value);
                for s in &e.body {
                    walk_stmt(s, ctx);
                }
            }
            if let Some(eb) = else_branch {
                for s in eb {
                    walk_stmt(s, ctx);
                }
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, ctx, Place::Value);
            for arm in arms {
                walk_pattern(&arm.pattern, ctx);
                if let Some(g) = &arm.guard {
                    walk_expr(g, ctx, Place::Value);
                }
                for s in &arm.body {
                    walk_stmt(s, ctx);
                }
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(a, b, _) => {
                    walk_expr(a, ctx, Place::Value);
                    walk_expr(b, ctx, Place::Value);
                }
                TypedForIter::Expr(e) => walk_expr(e, ctx, Place::Value),
            }
            for s in body {
                walk_stmt(s, ctx);
            }
        }
        TypedStmtKind::While { cond, body, .. } => {
            walk_expr(cond, ctx, Place::Value);
            for s in body {
                walk_stmt(s, ctx);
            }
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(opt) => {
            if let Some(e) = opt {
                walk_expr(e, ctx, Place::Value);
            }
        }
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            walk_expr(cond, ctx, Place::Value);
            if let Some(m) = message {
                walk_expr(m, ctx, Place::Value);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            crate::sema::typed::TypedDeferBody::Expr(e) => walk_expr(e, ctx, Place::Value),
            crate::sema::typed::TypedDeferBody::Suite(stmts) => {
                for s in stmts {
                    walk_stmt(s, ctx);
                }
            }
        },
        TypedStmtKind::ExprStmt(e) => walk_expr(e, ctx, Place::Value),
        TypedStmtKind::BareSend { expr, .. } => walk_expr(expr, ctx, Place::Value),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                walk_expr(c, ctx, Place::Value);
            }
            if let Some(d) = deadline {
                walk_expr(d, ctx, Place::Value);
            }
            for s in body {
                walk_stmt(s, ctx);
            }
        }
    }
}

fn walk_pattern(p: &TypedPattern, ctx: &mut WalkCtx<'_>) {
    // Revision 0.1 has no struct-field patterns; keep the walk so a future
    // shape is not silently dropped from the census.
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => walk_expr(e, ctx, Place::Value),
        TypedPatternKind::Take(inner) => walk_pattern(inner, ctx),
        TypedPatternKind::Variant { payload, .. } => {
            for inner in payload {
                walk_pattern(inner, ctx);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for inner in items {
                walk_pattern(inner, ctx);
            }
        }
        TypedPatternKind::Or(alts) => {
            for inner in alts {
                walk_pattern(inner, ctx);
            }
        }
    }
    let _ = ctx;
}

fn walk_expr(e: &TypedExpr, ctx: &mut WalkCtx<'_>, place: Place) {
    match &e.kind {
        TypedExprKind::Field(base, name) => {
            if let Type::Named(sname, _) = unwrap_named(&base.ty) {
                match place {
                    Place::AssignTarget => ctx.record(Kind::Write, sname, name),
                    Place::Value => ctx.record(Kind::Read, sname, name),
                }
            }
            walk_expr(base, ctx, Place::Value);
        }
        TypedExprKind::StructLiteral { name, fields } => {
            for (fname, val) in fields {
                ctx.record(Kind::Construct, name, fname);
                walk_expr(val, ctx, Place::Value);
            }
        }
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
        | TypedExprKind::FnRef(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::GroupChild(_) => {}
        TypedExprKind::Index(base, idx) => {
            walk_expr(base, ctx, Place::Value);
            walk_expr(idx, ctx, Place::Value);
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_expr(r, ctx, Place::Value);
            }
            for a in args {
                if let Some(a) = a {
                    walk_expr(a, ctx, Place::Value);
                }
            }
        }
        TypedExprKind::CallValue(f, args) => {
            walk_expr(f, ctx, Place::Value);
            for a in args {
                walk_expr(a, ctx, Place::Value);
            }
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Try(inner, _)
        | TypedExprKind::Not(inner)
        | TypedExprKind::Panic(inner)
        | TypedExprKind::Await(inner)
        | TypedExprKind::Send(inner) => walk_expr(inner, ctx, Place::Value),
        TypedExprKind::Binary(_, a, b)
        | TypedExprKind::OpCall(_, a, b)
        | TypedExprKind::And(a, b)
        | TypedExprKind::Or(a, b) => {
            walk_expr(a, ctx, Place::Value);
            walk_expr(b, ctx, Place::Value);
        }
        TypedExprKind::Is(scrut, pat) => {
            walk_expr(scrut, ctx, Place::Value);
            walk_pattern(pat, ctx);
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                walk_expr(a, ctx, Place::Value);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => walk_expr(e, ctx, Place::Value),
            TypedClosureBody::Suite(stmts) => {
                for s in stmts {
                    walk_stmt(s, ctx);
                }
            }
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                walk_expr(i, ctx, Place::Value);
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_expr(r, ctx, Place::Value);
            }
            for (_, a) in args {
                walk_expr(a, ctx, Place::Value);
            }
        }
    }
}

fn unwrap_named(ty: &Type) -> &Type {
    match ty {
        Type::Own(_, inner) | Type::Static(inner) => unwrap_named(inner),
        other => other,
    }
}

/// Load + check a package root and return its field-visibility census.
pub fn census_root(root_file: &Path) -> Result<Census, String> {
    let loaded = crate::loader::load_closure(root_file).map_err(|e| match e {
        crate::loader::LoadError::Lex(e) => {
            format!("error[lex]: {} at {}:{}", e.message, e.line, e.col)
        }
        crate::loader::LoadError::Parse(e) => {
            format!("error[parse]: {} at {}:{}", e.message, e.line, e.col)
        }
        crate::loader::LoadError::Build(e) => {
            format!(
                "error[{}]: {} at {}:{}",
                e.category, e.message, e.line, e.col
            )
        }
    })?;
    let mut modules = BTreeMap::new();
    let mut paths = BTreeMap::new();
    for (key, m) in &loaded.modules {
        modules.insert(key.clone(), m.module.clone());
        paths.insert(key.clone(), m.file.display().to_string());
    }
    crate::sema::census_field_visibility(&modules, &paths)
        .map_err(|e| format!("error[{}]: {}", e.category, e.message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_shape_pins_summary_and_detail_lines() {
        let census = Census {
            violations: vec![
                Violation {
                    kind: Kind::Construct,
                    use_module: "app.user".into(),
                    decl_module: "lib.sealed".into(),
                    struct_name: "Sealed".into(),
                    field: "hidden".into(),
                    decl_struct: None,
                },
                Violation {
                    kind: Kind::Read,
                    use_module: "app.user".into(),
                    decl_module: "lib.sealed".into(),
                    struct_name: "Sealed".into(),
                    field: "hidden".into(),
                    decl_struct: None,
                },
            ],
        };
        let text = census.render();
        assert!(
            text.starts_with(
                "FieldVisibilityCensus constructs=1 reads=1 writes=0 patterns=0 fields=1\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("  construct use=app.user decl=lib.sealed struct=Sealed field=hidden\n"),
            "{text}"
        );
        assert!(
            text.contains("  read use=app.user decl=lib.sealed struct=Sealed field=hidden\n"),
            "{text}"
        );
    }
}
