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
use crate::sema::typed::TypedProgram;
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
// 05-library.md §9, in full: an actor declaration's arguments "must match
// `A.init` **(or its literal constructor)** after generated capabilities
// and handles are substituted". Both halves of that parenthetical are
// live (`find_constructor`, below): a struct that declares an `init` is
// matched against its parameters; a struct that declares none is
// constructed by its *literal* constructor and is matched against its own
// declared fields, name and declared type alike. The field half was
// missing until 2026-07-24 — `find_init`'s `None` arm handed the loop an
// empty parameter slice, so *every* non-reserved wiring argument to a
// no-`init` struct was rejected as naming nothing, and the three actor
// boot goldens (`boot-actors`, `boot-actor-chain`,
// `boot-actor-reply-struct`) could not reach `--stage=image`/`report`/
// `wrela build` at all despite running correctly under `wrela test`.
// The widening is exactly the doc sentence and no wider: an argument
// naming neither an `init` parameter nor a declared field is still an
// error, and a value that does not fit the field's declared type is
// still an error naming that field.
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

/// The labels an `img.actor(...)` call owns as image wiring rather than as
/// constructor arguments — the decision-7 sub-note above, made available to
/// the one other pass that has to agree with it.
///
/// plans/M7.md item W: `layout::build_boot_init_calls` materializes those
/// same arguments into boot's own `init` call and must skip exactly the set
/// this module skips when it accepts them. Sharing the predicate rather
/// than restating the two labels is the whole point — a divergence would
/// mean an argument accepted here and dropped there (or the reverse), which
/// is the precise failure mode item W exists to close.
pub(crate) fn is_reserved_actor_arg(label: &str) -> bool {
    reserved_args(DeclKind::Actor).contains(&label)
}

/// Shared with `layout::build_boot_init_calls` for the same reason
/// `is_reserved_actor_arg` above is: this pass *accepts* a parameter of
/// one of these types with no explicit argument (decision 7's own
/// substitution rule), and boot has to recognize exactly the same set to
/// fail closed on it by name until plans/M7.md item A mints one. Two
/// copies of this list could disagree; one cannot.
pub(crate) fn is_capability_type_name(name: &str) -> bool {
    matches!(name, "DeviceCap" | "DmaPool" | "Mmio" | "IrqCap")
}

pub(crate) fn is_handle_type_name(name: &str) -> bool {
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

/// The name/type slots an image wiring argument is matched against —
/// 05-library.md §9's own "must match `A.init` **(or its literal
/// constructor)**", made mechanical. A struct that declares an `init` is
/// constructed by that `init`, so its parameters are the slots; a struct
/// that declares none is constructed by its *literal* constructor — its
/// own declared fields — so the fields are the slots, name and declared
/// type alike, in declaration order.
///
/// The two are not interchangeable in the diagnostics (a message naming
/// `X.init` for a struct that has no `init` would name something that
/// does not exist) nor in the *missing*-slot direction — see
/// `check_one_decl` below, which runs that direction only for `Init`.
enum Constructor {
    Init(Vec<(String, Type)>),
    Fields(Vec<(String, Type)>),
}

impl Constructor {
    fn slots(&self) -> &[(String, Type)] {
        match self {
            Constructor::Init(s) | Constructor::Fields(s) => s,
        }
    }
}

/// `05-library.md` §9's own "`A.init` (or its literal constructor)",
/// resolved over the whole build closure (an actor/driver struct's own
/// declaration may live in a different module than the `@image` fn that
/// wires it — `check_init_args`'s own doc comment).
///
/// The `init` search is exactly what it always was: the first module (in
/// `programs`'s own BTree order) whose same-named struct declares one
/// wins. Only when *no* module's copy declares an `init` at all does the
/// literal-constructor arm run, over the first module declaring the
/// struct.
///
/// A struct name no module in the closure declares at all falls back to
/// `Constructor::Init(vec![])` — byte-identical to the pre-existing
/// behavior (every ordinary argument rejected by name, no missing-slot
/// error), and still the honest wording: today the only shape that can
/// reach here unfound is a *generic* instantiation, whose checked body
/// lives in `TypedProgram::instantiations` rather than `structs`, and
/// whose `init` is therefore invisible to this pass. Fail closed, and
/// leave it exactly as loud as it already was.
fn find_constructor(programs: &BTreeMap<String, TypedProgram>, struct_name: &str) -> Constructor {
    if let Some(f) = programs
        .values()
        .find_map(|p| p.structs.get(struct_name).and_then(|s| s.init.as_ref()))
    {
        return Constructor::Init(
            f.params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect(),
        );
    }
    let Some(s) = programs.values().find_map(|p| p.structs.get(struct_name)) else {
        return Constructor::Init(Vec::new());
    };
    Constructor::Fields(
        s.fields
            .iter()
            .filter_map(|name| s.field_types.get(name).map(|ty| (name.clone(), ty.clone())))
            .collect(),
    )
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
    let ctor = find_constructor(programs, struct_name);
    let reserved = reserved_args(kind);
    let mut satisfied: BTreeSet<String> = BTreeSet::new();
    for a in args {
        if reserved.contains(&a.label.as_str()) {
            continue;
        }
        let Some((slot_name, slot_ty)) = ctor.slots().iter().find(|(n, _)| *n == a.label) else {
            return Err(build_error(match &ctor {
                Constructor::Init(_) => format!(
                    "`{}` passes `{}=...` to `{struct_name}`, but `{struct_name}.init` has no \
                     parameter named `{}`",
                    decl_ref.render(),
                    a.label,
                    a.label
                ),
                Constructor::Fields(_) => format!(
                    "`{}` passes `{}=...` to `{struct_name}`, but `{struct_name}` declares no \
                     `init` and has no field named `{}`",
                    decl_ref.render(),
                    a.label,
                    a.label
                ),
            }));
        };
        if matches!(a.value, Value::ImageDecl(_)) {
            satisfied.insert(slot_name.clone());
            continue;
        }
        if !value_fits_param_type(a, slot_ty) {
            return Err(build_error(match &ctor {
                Constructor::Init(_) => format!(
                    "`{}` passes `{}={}`, which does not fit `{struct_name}.init`'s own `{}: {}`",
                    decl_ref.render(),
                    a.label,
                    types::render_type(&a.ty),
                    slot_name,
                    types::render_type(slot_ty)
                ),
                Constructor::Fields(_) => format!(
                    "`{}` passes `{}={}`, which does not fit `{struct_name}`'s own field `{}: {}`",
                    decl_ref.render(),
                    a.label,
                    types::render_type(&a.ty),
                    slot_name,
                    types::render_type(slot_ty)
                ),
            }));
        }
        satisfied.insert(slot_name.clone());
    }
    // The *missing*-slot direction, for a declared `init` only. A field
    // this image leaves unwired is not missing anything: the boot
    // sequence zero-initializes every actor's whole state slot before any
    // turn runs and only then calls a declared zero-argument `init`
    // (`layout::build_boot_init`'s own doc comment, whose disclosed floor
    // says materializing `ActorDecl::args` against a real parameter list
    // is deferred work) — so an unsupplied field has a defined value, and
    // demanding an argument for it would reject every actor whose state
    // is ordinary data (`tests/golden/boot-actors`'s own `Ledger.marks:
    // u64`, wired with nothing but the reserved `mailbox=`). 05-library.md
    // §9's own "a resource `init` argument without one recovery source is
    // a build error" names `init` arguments, and that is exactly the
    // scope kept here.
    let Constructor::Init(params) = &ctor else {
        return Ok(());
    };
    for (name, ty) in params {
        if satisfied.contains(name) {
            continue;
        }
        if let Type::Named(tn, _) = ty {
            if is_capability_type_name(tn) || is_handle_type_name(tn) {
                continue;
            }
        }
        return Err(build_error(format!(
            "`{}` is missing `{struct_name}.init`'s own `{}: {}` — no `{}=...` argument and no \
             wiring substitution source",
            decl_ref.render(),
            name,
            types::render_type(ty),
            name
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
    use crate::sema::typed::{TypedFn, TypedParam, TypedStruct};
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

    // --- the literal-constructor half (05-library.md §9: "or its literal
    // constructor") — a struct that declares no `init` at all ----------------

    fn program_with_fields(struct_name: &str, fields: Vec<(&str, Type)>) -> TypedProgram {
        let mut program = TypedProgram::default();
        program.structs.insert(
            struct_name.to_string(),
            TypedStruct {
                name: struct_name.to_string(),
                fields: fields.iter().map(|(n, _)| n.to_string()).collect(),
                field_types: fields
                    .into_iter()
                    .map(|(n, t)| (n.to_string(), t))
                    .collect(),
                init: None,
                ..TypedStruct::default()
            },
        );
        program
    }

    #[test]
    fn a_handle_argument_matches_a_declared_field_when_the_struct_has_no_init() {
        // `tests/golden/image-field-wired-accept`'s own shape, in
        // miniature: `Worker.led: Actor[Ledger]`, no `init`, wired
        // `led=<actor#0>`. Before the literal-constructor half landed
        // this was rejected outright — a no-`init` struct was handed an
        // empty parameter list, so every wiring argument named nothing.
        let programs = programs_map(program_with_fields(
            "Worker",
            vec![("led", Type::Named("Actor".to_string(), vec![]))],
        ));
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Worker".to_string(), vec![]),
            args: vec![decl_arg(
                "led",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Actor(0)),
            )],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    #[test]
    fn an_unwired_field_is_never_a_missing_init_argument() {
        // `tests/golden/boot-actors`'s own `Ledger.marks: u64`: a plain
        // data field, no `init`, wired with nothing. The missing-slot
        // direction runs for a declared `init` only (`check_one_decl`'s
        // own comment) — the boot sequence zero-initializes the whole
        // state slot, so there is nothing missing here.
        let programs = programs_map(program_with_fields("Ledger", vec![("marks", Type::U64)]));
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Ledger".to_string(), vec![]),
            args: vec![],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    #[test]
    fn an_argument_naming_neither_an_init_param_nor_a_field_is_rejected() {
        // `tests/golden/err-image-field-unknown`'s own shape: the
        // widening is exactly 05 §9's sentence, never "anything goes".
        let programs = programs_map(program_with_fields("Store", vec![("value", Type::U64)]));
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![decl_arg("queue_depth", Type::I64, Value::I64(8))],
        });
        let err = check_init_args(&g, &programs).expect_err("Store has no `queue_depth` field");
        assert_eq!(err.category, "build");
        assert!(err.message.contains("queue_depth"));
        assert!(
            err.message
                .contains("declares no `init` and has no field named")
        );
    }

    #[test]
    fn a_literal_argument_is_range_checked_against_its_field_type() {
        // The same `value_fits_param_type` accommodation the `init` half
        // already needed (a builder intrinsic argument is typed with no
        // expected type, so `seed=8` is recorded at its `i64` default),
        // reaching a *field*'s declared type instead of a parameter's.
        let programs = programs_map(program_with_fields("Store", vec![("seed", Type::U32)]));
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![decl_arg("seed", Type::I64, Value::I64(8))],
        });
        assert!(check_init_args(&g, &programs).is_ok());
    }

    #[test]
    fn a_value_that_does_not_fit_its_field_type_names_the_field() {
        let programs = programs_map(program_with_fields("Store", vec![("seed", Type::U8)]));
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![decl_arg("seed", Type::I64, Value::I64(4096))],
        });
        let err = check_init_args(&g, &programs).expect_err("4096 does not fit a u8 field");
        assert!(err.message.contains("own field `seed: u8`"));
        // Never `Store.init` — `Store` declares no `init` at all.
        assert!(!err.message.contains(".init"));
    }

    #[test]
    fn a_declared_init_still_wins_over_the_fields_of_the_same_struct() {
        // 05 §9's "or" is exclusive: a struct that declares an `init` is
        // matched against that `init`, and a field it does not name is
        // not a wiring slot.
        let mut program = program_with_init(
            "Blk",
            vec![TypedParam {
                mode: AccessMode::Read,
                name: "queue_depth".to_string(),
                ty: Type::U32,
                default: None,
            }],
        );
        let s = program
            .structs
            .get_mut("Blk")
            .expect("Blk was just inserted");
        s.fields = vec!["id".to_string()];
        s.field_types = BTreeMap::from([("id".to_string(), Type::U32)]);
        let programs = programs_map(program);
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![
                decl_arg("queue_depth", Type::I64, Value::I64(8)),
                decl_arg("id", Type::I64, Value::I64(1)),
            ],
        });
        let err = check_init_args(&g, &programs).expect_err("`id` is a field, not an init param");
        assert!(
            err.message
                .contains("`Blk.init` has no parameter named `id`")
        );
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
