//! Graph checks (plans/M4.md item C, decisions 6/7): plain functions over
//! an already-sealed `eval::image::ImageGraph` — no traits, no registry,
//! `BTreeMap`/`Vec` throughout, fail-fast in one fixed documented order
//! (`check_sealed`, below). This is a *post-evaluation pass*: it runs once
//! (`bin/wrela.rs::run_image_stage`) after `eval::interp::eval_image`
//! already produced a sealed graph, rather than interleaving checks into
//! the intrinsics themselves (decision 5's own recorders already reject
//! the one thing they can name honestly on their own — a pool bound twice
//! by the same call chain — see `eval::image::ImageGraph::declare_pool`'s
//! own doc comment; everything else here needs the *whole* finished graph
//! at once, which only exists after the `@image` fn's body has fully run).
//!
//! Fixed check order (sub-note recorded at item C execution, 2026-07-23):
//! construction DAG, then pools bound-at-seal, then init-argument
//! matching, then supervision — first failure wins. Rationale: the DAG
//! check is the most structural (it does not even need to know what a
//! declaration *is*, only what it references) so it runs first; pool
//! binding is the next-most-structural fact (whether a declared resource
//! exists at all) and is a precondition for init-argument matching to
//! mean anything (an init argument can reference a pool-backed handle);
//! init-argument matching is the deepest per-declaration check; placement
//! in the supervision tree is the most "external" fact (it says nothing
//! about a declaration's own construction, only about the tree drawn over
//! already-valid declarations), so it runs last. `img.seal()`'s own
//! "every declaration is fully bound" (05-library.md §9) is exactly the
//! conjunction of every check below, not a separate mechanism —
//! `check_sealed` *is* the seal check; `image.graph.seal-fully-bound`'s
//! own ledger clause cites the same evidence as the others.
//!
//! One-image (decision 6's own "zero or more than one is a named
//! diagnostic listing every candidate") is deliberately *not* here: it is
//! decided before any `@image` fn is ever evaluated (`bin/wrela.rs`'s own
//! `run_image_stage`, growing item B's minimal slice), so there is no
//! `ImageGraph` yet for a plain function over one to check.
//!
//! `image.graph.dma-pools` (decision 10, explicit gap): `img.dma_pool`
//! already fails the whole build closed at evaluation time (`ImageGraph::declare_dma_pool`),
//! so `ImageGraph::dma_pools` is always empty by the time any check here
//! runs — nothing below needs its own DMA-specific case yet.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::image::{DeclArg, ImageDeclRef, ImageGraph};
use crate::eval::value::Value;
use crate::sema::SemaError;
use crate::sema::typed::{TypedFn, TypedParam, TypedProgram};
use crate::sema::types::{self, Type};

fn build_error(message: String) -> SemaError {
    SemaError {
        category: "build",
        message,
        line: 0,
        col: 0,
        extra_lines: Vec::new(),
        omit_location: true,
        missing_method: None,
    }
}

fn build_error_with_lines(message: String, extra_lines: Vec<String>) -> SemaError {
    SemaError {
        category: "build",
        message,
        line: 0,
        col: 0,
        extra_lines,
        omit_location: true,
        missing_method: None,
    }
}

/// The whole post-seal pass (this module's own doc comment: the fixed
/// order). `owner` is the module whose own `@image` fn produced `graph`
/// (its `declared_pools` is what check 3/6 needs — a source-declared
/// `pool` a module never binds leaves no trace in the graph itself);
/// `programs` is every module the build closure checked (an actor/driver
/// struct's own `init` may live in a different module than the `@image`
/// fn that wires it, plans/M4.md item A's own splice — see
/// `check_init_args`'s own doc comment).
pub fn check_sealed(
    graph: &ImageGraph,
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    check_construction_dag(graph)?;
    check_pools_bound(graph, &owner.declared_pools)?;
    check_init_args(graph, programs)?;
    check_supervision(graph)?;
    Ok(())
}

// --- shared: finding every `ImageDecl` value nested inside an already-
// evaluated argument value (an array of children, a single handle, ...) --

fn decl_refs_in_value(v: &Value, out: &mut Vec<ImageDeclRef>) {
    match v {
        Value::ImageDecl(r) => out.push(r.clone()),
        Value::Array(items) | Value::Tuple(items) | Value::Struct(items) => {
            for it in items {
                decl_refs_in_value(it, out);
            }
        }
        Value::Enum(_, payload) => {
            for p in payload {
                decl_refs_in_value(p, out);
            }
        }
        _ => {}
    }
}

// --- check 2: construction DAG (02-language.md §12.1, image.graph.construction-dag) --
//
// "Construction edges (moves, initialization order) must form a DAG;
// handle edges may be cyclic." Once evaluated, a `decl.handle()` call and
// a bare decl-reference argument are the identical `Value::ImageDecl`
// (`ImageDecl.handle`'s own eval arm is a pure passthrough of its
// receiver, `eval/interp.rs`'s own doc comment) — there is no way to tell
// which spelling a source argument used from the value alone. This check
// therefore treats every decl-reference argument as a construction edge,
// conservatively: since a local can only ever be assigned from an earlier
// declaration's own return value (straight-line evaluation, program
// order), every `ImageDeclRef` a later declaration's arguments can
// possibly name already exists earlier in this same graph's own
// construction order — so a real, source-evaluated `ImageGraph` can never
// actually contain a cycle, "handle" or otherwise, no matter how this
// check treats the two. The check is still implemented for real (dumb and
// correct, not skipped) and is exercised by this module's own hand-built
// `ImageGraph` unit tests below, which are free to wire a "later" index's
// argument back to reference an "earlier" one out of order — the one way
// to construct a cycle at all, and unrepresentable in today's language
// surface (no forward references exist yet). `image.graph.construction-dag`'s
// own ledger clause records this honestly: this check is real code, real
// unit-tested, and has no source-level failing golden — by construction,
// not because the check was skipped.
fn identified_decls(graph: &ImageGraph) -> Vec<(ImageDeclRef, &[DeclArg])> {
    let mut out = Vec::new();
    for (i, d) in graph.devices.iter().enumerate() {
        out.push((ImageDeclRef::Device(i), d.args.as_slice()));
    }
    for (i, d) in graph.drivers.iter().enumerate() {
        out.push((ImageDeclRef::Driver(i), d.args.as_slice()));
    }
    for (i, d) in graph.actors.iter().enumerate() {
        out.push((ImageDeclRef::Actor(i), d.args.as_slice()));
    }
    for (name, d) in &graph.pools {
        out.push((ImageDeclRef::Pool(name.clone()), d.args.as_slice()));
    }
    for (name, d) in &graph.dma_pools {
        out.push((ImageDeclRef::DmaPool(name.clone()), d.args.as_slice()));
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Visiting,
    Done,
}

fn visit_dag(
    node: &ImageDeclRef,
    edges: &BTreeMap<ImageDeclRef, Vec<ImageDeclRef>>,
    marks: &mut BTreeMap<ImageDeclRef, Mark>,
    path: &mut Vec<ImageDeclRef>,
) -> Result<(), Vec<ImageDeclRef>> {
    match marks.get(node) {
        Some(Mark::Done) => return Ok(()),
        Some(Mark::Visiting) => {
            let start = path
                .iter()
                .position(|n| n == node)
                .expect("a node marked Visiting is always still on the path");
            let mut cycle: Vec<ImageDeclRef> = path[start..].to_vec();
            cycle.push(node.clone());
            return Err(cycle);
        }
        None => {}
    }
    marks.insert(node.clone(), Mark::Visiting);
    path.push(node.clone());
    if let Some(deps) = edges.get(node) {
        for dep in deps {
            visit_dag(dep, edges, marks, path)?;
        }
    }
    path.pop();
    marks.insert(node.clone(), Mark::Done);
    Ok(())
}

pub fn check_construction_dag(graph: &ImageGraph) -> Result<(), SemaError> {
    let nodes = identified_decls(graph);
    let mut edges: BTreeMap<ImageDeclRef, Vec<ImageDeclRef>> = BTreeMap::new();
    for (id, args) in &nodes {
        let mut refs = Vec::new();
        for a in *args {
            decl_refs_in_value(&a.value, &mut refs);
        }
        edges.insert(id.clone(), refs);
    }

    let mut marks: BTreeMap<ImageDeclRef, Mark> = BTreeMap::new();
    let mut path: Vec<ImageDeclRef> = Vec::new();
    for (id, _) in &nodes {
        if marks.contains_key(id) {
            continue;
        }
        if let Err(cycle) = visit_dag(id, &edges, &mut marks, &mut path) {
            let lines: Vec<String> = cycle
                .windows(2)
                .map(|w| format!("  {} -> {}", w[0].render(), w[1].render()))
                .collect();
            return Err(build_error_with_lines(
                "construction cycle detected among image declarations (02-language.md §12.1: \
                 construction edges must form a DAG)"
                    .to_string(),
                lines,
            ));
        }
    }
    Ok(())
}

// --- check 3/6: every source-declared pool bound exactly once
// (05-library.md §9, image.graph.pools-bound-once/seal-fully-bound) -------
//
// A pool bound *twice* is already rejected the instant the second
// `img.pool`/`img.dma_pool` call runs (`ImageGraph::declare_pool`'s own
// `Err` — the graph can never even be built with two entries under the
// same key); binding a name with no `pool P` declaration at all is
// already rejected earlier still, at typing (`sema::bodies::check_intrinsic_args`
// only ever recognizes a *declared* pool name as a `PoolName` leaf — an
// undeclared one falls through to the ordinary name resolver and fails
// there, `error[type]: cannot determine the type of ...`). The one case
// left for a post-seal check is a `pool P` declared in source that is
// never bound by *any* call before `img.seal()` — the graph itself only
// ever records bound pools, so this is the one pool fact only the
// declaring module's own `declared_pools` can supply.
pub fn check_pools_bound(
    graph: &ImageGraph,
    declared_pools: &BTreeSet<String>,
) -> Result<(), SemaError> {
    if let Some(name) = declared_pools
        .iter()
        .find(|name| !graph.pools.contains_key(*name) && !graph.dma_pools.contains_key(*name))
    {
        return Err(build_error(format!(
            "pool `{name}` is declared but never bound by `img.pool`/`img.dma_pool` before \
             `img.seal()`"
        )));
    }
    Ok(())
}

// --- check 4/7: init-argument matching with substitution
// (05-library.md §9, image.graph.init-args-match) --------------------------
//
// Decision-7 sub-note (recorded at item C execution, 2026-07-23 — the
// plan's own "decide the dumb exact rule, record it" instruction): the
// intrinsic's own *reserved* arguments belong to the image's wiring, not
// to the actor/driver's `init` parameter list, and are skipped entirely
// before any parameter matching runs — `device`/`core` for `img.driver`,
// `mailbox`/`core` for `img.actor` (`reserved_args`, below). Every other
// labeled argument must name a same-named `init` parameter (an argument
// naming none is an error — `err-image-init-extra-arg`); an argument
// whose evaluated value is itself a decl reference (`Value::ImageDecl`,
// with or without its own `.handle()` — indistinguishable once evaluated,
// this module's own construction-DAG doc comment) is accepted for that
// parameter with no further type check (`ImageDecl` is opaque — this is
// the most this milestone can verify of a "handle" substitution); any
// other argument's evaluated value must numerically/structurally fit the
// parameter's declared type (`value_fits_param_type`, below — the one
// place this reaches past the argument's own *recorded* static type,
// which a builder intrinsic's no-expected-type checking leaves at its
// no-context default for a bare integer literal, `sema::bodies::check_intrinsic_args`'s
// own doc comment — everything else keeps a plain type equality check).
// Every `init` parameter not covered by an explicit argument above must
// still be satisfiable by a *substitution* source to be legal: a
// parameter whose declared type is named `DeviceCap`/`DmaPool`/`Mmio`/
// `IrqCap` (recognized by name, decision 7) is satisfied by the
// declaration's own device wiring; a parameter named `Actor` is satisfied
// by an actor handle the same way — neither of these type names resolves
// as a real type annotation anywhere in this compiler yet
// (`sema::types::resolve_named`'s own fixed arm list has no case for any
// of them), so this half of the rule is currently unrepresentable from
// real source and is exercised only by this module's own hand-built unit
// tests below, exactly like the construction-DAG cycle case. A parameter
// satisfied by neither an explicit argument nor a recognized substitution
// source is an error (`err-image-init-missing-arg`) — 05-library.md §9's
// own "a resource `init` argument without one recovery source is a build
// error" falls out of this exact same case (decision 7's own sub-note:
// no separate mechanism exists or is needed, `image.graph.init-args-match`'s
// own ledger clause covers both).
#[derive(Clone, Copy)]
enum DeclKind {
    Driver,
    Actor,
}

fn reserved_args(kind: DeclKind) -> &'static [&'static str] {
    match kind {
        DeclKind::Driver => &["device", "core"],
        DeclKind::Actor => &["mailbox", "core"],
    }
}

fn is_capability_type_name(name: &str) -> bool {
    matches!(name, "DeviceCap" | "DmaPool" | "Mmio" | "IrqCap")
}

fn is_handle_type_name(name: &str) -> bool {
    name == "Actor"
}

fn int_value_as_i128(v: &Value) -> Option<i128> {
    Some(match v {
        Value::U8(n) => *n as i128,
        Value::U16(n) => *n as i128,
        Value::U32(n) => *n as i128,
        Value::U64(n) => *n as i128,
        Value::Usize(n) => *n as i128,
        Value::I8(n) => *n as i128,
        Value::I16(n) => *n as i128,
        Value::I32(n) => *n as i128,
        Value::I64(n) => *n as i128,
        Value::Isize(n) => *n as i128,
        _ => return None,
    })
}

fn int_bounds(ty: &Type) -> Option<(i128, i128)> {
    Some(match ty {
        Type::U8 => (0, u8::MAX as i128),
        Type::U16 => (0, u16::MAX as i128),
        Type::U32 => (0, u32::MAX as i128),
        Type::U64 => (0, u64::MAX as i128),
        Type::Usize => (0, u64::MAX as i128),
        Type::I8 => (i8::MIN as i128, i8::MAX as i128),
        Type::I16 => (i16::MIN as i128, i16::MAX as i128),
        Type::I32 => (i32::MIN as i128, i32::MAX as i128),
        Type::I64 => (i64::MIN as i128, i64::MAX as i128),
        Type::Isize => (i64::MIN as i128, i64::MAX as i128),
        _ => return None,
    })
}

fn value_fits_param_type(arg: &DeclArg, param_ty: &Type) -> bool {
    if &arg.ty == param_ty {
        return true;
    }
    match (int_value_as_i128(&arg.value), int_bounds(param_ty)) {
        (Some(v), Some((lo, hi))) => v >= lo && v <= hi,
        _ => false,
    }
}

fn find_init<'p>(
    programs: &'p BTreeMap<String, TypedProgram>,
    struct_name: &str,
) -> Option<&'p TypedFn> {
    programs
        .values()
        .find_map(|p| p.structs.get(struct_name).and_then(|s| s.init.as_ref()))
}

fn check_one_decl(
    decl_ref: &ImageDeclRef,
    actor_type: &Type,
    args: &[DeclArg],
    kind: DeclKind,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    let Type::Named(struct_name, _) = actor_type else {
        return Ok(()); // defensive: only ever a bare struct name reaches here
    };
    let empty: Vec<TypedParam> = Vec::new();
    let params: &[TypedParam] = match find_init(programs, struct_name) {
        Some(f) => &f.params,
        None => &empty,
    };
    let reserved = reserved_args(kind);
    let mut satisfied: BTreeSet<String> = BTreeSet::new();
    for a in args {
        if reserved.contains(&a.label.as_str()) {
            continue;
        }
        let Some(p) = params.iter().find(|p| p.name == a.label) else {
            return Err(build_error(format!(
                "`{}` passes `{}=...` to `{struct_name}`, but `{struct_name}.init` has no \
                 parameter named `{}`",
                decl_ref.render(),
                a.label,
                a.label
            )));
        };
        if matches!(a.value, Value::ImageDecl(_)) {
            satisfied.insert(p.name.clone());
            continue;
        }
        if !value_fits_param_type(a, &p.ty) {
            return Err(build_error(format!(
                "`{}` passes `{}={}`, which does not fit `{struct_name}.init`'s own `{}: {}`",
                decl_ref.render(),
                a.label,
                types::render_type(&a.ty),
                p.name,
                types::render_type(&p.ty)
            )));
        }
        satisfied.insert(p.name.clone());
    }
    for p in params {
        if satisfied.contains(&p.name) {
            continue;
        }
        if let Type::Named(name, _) = &p.ty {
            if is_capability_type_name(name) || is_handle_type_name(name) {
                continue;
            }
        }
        return Err(build_error(format!(
            "`{}` is missing `{struct_name}.init`'s own `{}: {}` — no `{}=...` argument and no \
             wiring substitution source",
            decl_ref.render(),
            p.name,
            types::render_type(&p.ty),
            p.name
        )));
    }
    Ok(())
}

pub fn check_init_args(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    for (i, d) in graph.drivers.iter().enumerate() {
        check_one_decl(
            &ImageDeclRef::Driver(i),
            &d.actor_type,
            &d.args,
            DeclKind::Driver,
            programs,
        )?;
    }
    for (i, d) in graph.actors.iter().enumerate() {
        check_one_decl(
            &ImageDeclRef::Actor(i),
            &d.actor_type,
            &d.args,
            DeclKind::Actor,
            programs,
        )?;
    }
    Ok(())
}

// --- check 5: exactly one supervising parent
// (05-library.md §9, image.graph.supervision-one-parent) -------------------
//
// "Exactly one parent per actor/task" — every declared driver/actor must
// appear in exactly one `img.supervise(children=[...])` group's own
// `children` list. Devices and pools are never supervised (05-library.md
// §9 names only "actor/task"), so neither is a node here. All groups
// declared by an M4 `@image` fn are top-level (`img.supervise` returns
// `Unit`, decision 5 — there is nothing composable to nest a group under
// another), so "the image root is the implicit parent of top-level
// `supervise` groups" is trivially satisfied by every group this milestone
// can even construct: there is no nested-supervision surface yet for a
// group to *not* be top-level under, so this rule needs no separate check
// beyond "every driver/actor is in exactly one group" below.
pub fn check_supervision(graph: &ImageGraph) -> Result<(), SemaError> {
    let mut counts: BTreeMap<ImageDeclRef, usize> = BTreeMap::new();
    for s in &graph.supervisions {
        for a in &s.args {
            if a.label != "children" {
                continue;
            }
            let mut refs = Vec::new();
            decl_refs_in_value(&a.value, &mut refs);
            for r in refs {
                *counts.entry(r).or_insert(0) += 1;
            }
        }
    }
    let mut nodes: Vec<ImageDeclRef> = Vec::new();
    for i in 0..graph.drivers.len() {
        nodes.push(ImageDeclRef::Driver(i));
    }
    for i in 0..graph.actors.len() {
        nodes.push(ImageDeclRef::Actor(i));
    }
    for n in &nodes {
        match counts.get(n).copied().unwrap_or(0) {
            0 => {
                return Err(build_error(format!(
                    "`{}` is not supervised by any `img.supervise(children=[...])` group",
                    n.render()
                )));
            }
            1 => {}
            _ => {
                return Err(build_error(format!(
                    "`{}` is supervised by more than one `img.supervise(children=[...])` group",
                    n.render()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema::typed::TypedStruct;
    use crate::syntax::ast::AccessMode;

    fn decl_arg(label: &str, ty: Type, value: Value) -> DeclArg {
        DeclArg {
            label: label.to_string(),
            ty,
            value,
        }
    }

    // --- construction DAG -------------------------------------------------

    #[test]
    fn a_real_graph_with_only_backward_references_is_always_acyclic() {
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![],
        });
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![decl_arg(
                "disk",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Driver(0)),
            )],
        });
        assert!(check_construction_dag(&g).is_ok());
    }

    #[test]
    fn a_hand_built_cycle_is_rejected() {
        // Unrepresentable from real source (this module's own doc
        // comment): only a hand-built graph can wire an *earlier* index's
        // own arguments to reference a *later* one, the one way to close
        // a cycle at all.
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg(
                "peer",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Actor(0)),
            )],
        });
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![decl_arg(
                "peer",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Driver(0)),
            )],
        });
        let err = check_construction_dag(&g).expect_err("driver#0 <-> actor#0 is a cycle");
        assert_eq!(err.category, "build");
        assert!(err.message.contains("cycle"));
        assert!(
            !err.extra_lines.is_empty(),
            "the cycle prints one line per hop"
        );
    }

    // --- pools bound-once/at-seal ------------------------------------------

    #[test]
    fn an_unbound_declared_pool_is_rejected_at_seal() {
        let g = ImageGraph::default();
        let mut declared = BTreeSet::new();
        declared.insert("Buffers".to_string());
        let err =
            check_pools_bound(&g, &declared).expect_err("Buffers is declared but never bound");
        assert!(err.message.contains("Buffers"));
        assert!(err.message.contains("never bound"));
    }

    #[test]
    fn a_bound_declared_pool_passes() {
        let mut g = ImageGraph::default();
        g.pools.insert(
            "Buffers".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::U32,
                args: vec![],
            },
        );
        let mut declared = BTreeSet::new();
        declared.insert("Buffers".to_string());
        assert!(check_pools_bound(&g, &declared).is_ok());
    }

    // --- init-argument matching ---------------------------------------------

    fn program_with_init(struct_name: &str, params: Vec<TypedParam>) -> TypedProgram {
        let mut program = TypedProgram::default();
        program.structs.insert(
            struct_name.to_string(),
            TypedStruct {
                name: struct_name.to_string(),
                init: Some(TypedFn {
                    receiver: Some((
                        AccessMode::Mut,
                        Type::Named(struct_name.to_string(), vec![]),
                    )),
                    params,
                    ret: Type::Unit,
                    body: vec![],
                    is_async: false,
                }),
                ..TypedStruct::default()
            },
        );
        program
    }

    fn programs_map(program: TypedProgram) -> BTreeMap<String, TypedProgram> {
        let mut m = BTreeMap::new();
        m.insert("m".to_string(), program);
        m
    }

    #[test]
    fn an_extra_intrinsic_arg_naming_no_init_param_is_rejected() {
        let program = program_with_init("Blk", vec![]);
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg("queue_depth", Type::I64, Value::I64(8))],
        });
        let err = check_init_args(&g, &programs).expect_err("Blk.init takes no params");
        assert!(err.message.contains("queue_depth"));
        assert!(err.message.contains("no parameter named"));
    }

    #[test]
    fn a_missing_init_param_with_no_substitution_source_is_rejected() {
        let program = program_with_init(
            "Blk",
            vec![TypedParam {
                mode: AccessMode::Read,
                name: "queue_depth".to_string(),
                ty: Type::U32,
                default: None,
            }],
        );
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![],
        });
        let err = check_init_args(&g, &programs).expect_err("queue_depth has no source");
        assert!(err.message.contains("queue_depth"));
        assert!(err.message.contains("missing"));
    }

    #[test]
    fn an_int_literal_argument_fits_a_narrower_declared_param_type() {
        // `sema::bodies::check_intrinsic_args` types a bare integer
        // literal with no expected type (this module's own doc comment on
        // `value_fits_param_type`) — recorded here as `i64`, same as the
        // real evaluator would, against a `u32` init parameter.
        let program = program_with_init(
            "Blk",
            vec![TypedParam {
                mode: AccessMode::Read,
                name: "queue_depth".to_string(),
                ty: Type::U32,
                default: None,
            }],
        );
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg("queue_depth", Type::I64, Value::I64(8))],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    #[test]
    fn a_handle_argument_satisfies_its_named_param_with_no_type_check() {
        let program = program_with_init(
            "Store",
            vec![TypedParam {
                mode: AccessMode::Read,
                name: "disk".to_string(),
                ty: Type::Named("Blk".to_string(), vec![]),
                default: None,
            }],
        );
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![decl_arg(
                "disk",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Driver(0)),
            )],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    #[test]
    fn a_capability_typed_init_param_is_satisfied_by_name_with_no_argument() {
        // Unrepresentable from real source today (this module's own doc
        // comment: `DeviceCap[...]` never resolves as a type annotation) —
        // exercised here on a hand-built `TypedParam` only.
        let program = program_with_init(
            "Blk",
            vec![TypedParam {
                mode: AccessMode::Read,
                name: "cap".to_string(),
                ty: Type::Named("DeviceCap".to_string(), vec![]),
                default: None,
            }],
        );
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    #[test]
    fn reserved_args_are_never_matched_against_init_params() {
        let program = program_with_init("Blk", vec![]);
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.devices.push(crate::eval::image::DeviceDecl {
            device_type: Type::Named("NicHw".to_string(), vec![]),
            args: vec![],
        });
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg(
                "device",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Device(0)),
            )],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    // --- supervision ---------------------------------------------------------

    #[test]
    fn an_unsupervised_actor_is_named_as_an_orphan() {
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![],
        });
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![],
        });
        g.supervisions.push(crate::eval::image::SuperviseDecl {
            args: vec![decl_arg(
                "children",
                Type::Named("ImageDeclArray".to_string(), vec![]),
                Value::Array(vec![Value::ImageDecl(ImageDeclRef::Driver(0))]),
            )],
        });
        let err = check_supervision(&g).expect_err("actor#0 is never supervised");
        assert!(err.message.contains("actor#0"));
        assert!(err.message.contains("not supervised"));
    }

    #[test]
    fn a_doubly_supervised_driver_is_rejected() {
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![],
        });
        for _ in 0..2 {
            g.supervisions.push(crate::eval::image::SuperviseDecl {
                args: vec![decl_arg(
                    "children",
                    Type::Named("ImageDeclArray".to_string(), vec![]),
                    Value::Array(vec![Value::ImageDecl(ImageDeclRef::Driver(0))]),
                )],
            });
        }
        let err = check_supervision(&g).expect_err("driver#0 has two parents");
        assert!(err.message.contains("driver#0"));
        assert!(err.message.contains("more than one"));
    }
}
