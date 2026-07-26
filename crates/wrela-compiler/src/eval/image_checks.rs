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
//! Fixed check order (sub-note recorded at item C execution, 2026-07-23;
//! extended once, at plans/M7.md item D, by inserting the pool-declaration
//! check between pool binding and init-argument matching — a pool's own
//! declaration must be *valid* before an init argument that names its
//! handle can mean anything, which is the same reason binding already ran
//! before init matching; extended again by the post-closure M7 sweep
//! port, inserting device-bound-once before init-argument matching):
//! construction DAG, then pools bound-at-seal, then pool declarations,
//! then device-bound-once, then init-argument matching, then supervision
//! — first failure wins. Rationale: the DAG check is the most structural
//! (it does not even need to know what a declaration *is*, only what it
//! references) so it runs first; pool binding is the next-most-structural
//! fact (whether a declared resource exists at all) and is a precondition
//! for init-argument matching to mean anything (an init argument can
//! reference a pool-backed handle); device-bound-once is the mint's
//! per-device half of 03 §1's "named once" and must hold before any
//! `DeviceCap` is substituted; init-argument matching is the deepest
//! per-declaration check; placement in the supervision tree is the most
//! "external" fact (it says nothing about a declaration's own
//! construction, only about the tree drawn over already-valid
//! declarations), so it runs last. `img.seal()`'s own "every declaration
//! is fully bound" (05-library.md §9) is exactly the conjunction of every
//! check below, not a separate mechanism — `check_sealed` *is* the seal
//! check; `image.graph.seal-fully-bound`'s own ledger clause cites the
//! same evidence as the others.
//!
//! One-image (decision 6's own "zero or more than one is a named
//! diagnostic listing every candidate") is deliberately *not* here: it is
//! decided before any `@image` fn is ever evaluated (`bin/wrela.rs`'s own
//! `run_image_stage`, growing item B's minimal slice), so there is no
//! `ImageGraph` yet for a plain function over one to check.
//!
//! `image.graph.dma-pools` was an explicit gap (plans/M4.md decision 10)
//! for exactly as long as `img.dma_pool` failed the whole build closed at
//! evaluation time: `ImageGraph::dma_pools` was always empty by the time
//! any check here ran. plans/M7.md item D closes it — `check_pool_decls`
//! (below, third in the fixed order) is the DMA-specific case, and it is
//! where 03-hardware.md §3's own declared facts (a `@layout(dma)` payload,
//! a reachable device, an exact count, and therefore an exact size and
//! alignment) are decided once, for both the report and the placement.

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
    // plans/M7.md item D self-audit finding: this used to be handed the
    // `@image` fn's *own module's* declared pools only, while
    // `image.graph.pools-bound-once`'s own note already said the rule is
    // "a `pool P` declared in source that is never bound by *any* call
    // before `img.seal()`" and "only the declaring module's own
    // `declared_pools` can supply" it — for every declaring module, not
    // just one. A `pool Q` declared in another module of the closure, with
    // `own[Q] T` signatures over it and no `img.dma_pool`/`img.pool`
    // binding anywhere, sailed through. 02-language.md §4 is unambiguous:
    // "An image pool is bound to a pool name — a module- or actor-scoped
    // `pool Name` declaration that **the image binds** to exactly one pool
    // node."
    let mut declared: BTreeSet<String> = owner.declared_pools.clone();
    for p in programs.values() {
        declared.extend(p.declared_pools.iter().cloned());
    }
    check_pools_bound(graph, &declared)?;
    check_pool_decls(graph, programs)?;
    // Post-closure M7 sweep port: one device, one binding. Must run before
    // `check_init_args` minting — a second binding would otherwise mint a
    // second `DeviceCap` over a second register window for the same device.
    check_device_bound_once(graph)?;
    check_init_args(graph, programs)?;
    check_supervision(graph)?;
    // plans/M7.md item E1, decision 14: required features vs DEVICE_FEATURES
    // is a *build* error; capacity_sectors is the build constant
    // `read_capacity_sectors` lowers to.
    check_blk_device_decls(graph, programs)?;
    // plans/M8.md item P, decision 25: and that configuration belongs to
    // the device whose pool hosts the ring, not to whichever device
    // declared it first. Needs the resolved pool backings, which
    // `check_pool_decls` above has already proved well-formed.
    check_blk_config_names_the_blk_device(
        graph,
        programs,
        &pool_backings(graph, &closure_layouts(programs)?)?,
    )?;
    // DriverMode before vector-binding: an Irq build without `vector=`
    // would also trip `take_irq` unowned (§6); name the §7 mode
    // contradiction first when MODE is present.
    check_driver_mode(graph)?;
    check_vector_bindings(graph, programs)?;
    // plans/M8.md item B: `core=` range / virtio-blk pin — sizing and the
    // inferred table themselves live in `placement::place` (report/build),
    // which needs a LayoutCtx this pass does not have.
    crate::placement::check_annotations(graph).map_err(build_error)?;
    Ok(())
}

/// 03-hardware.md §1: "The device itself is named once, at the image
/// binding (`img.driver(BlkDriver, device=blk_device)`), the single source
/// of truth."
///
/// Item A's mint check enforces the *per-driver* half (at most one
/// `DeviceCap` parameter per binding). The *per-device* half — at most one
/// driver binding any given device — was only claimed, in
/// `layout::DeviceRegs`'s own comment ("`eval::image_checks` already
/// refuses a second binding of the same device, so this is not a list").
/// It was not enforced. Two `img.driver(..., device=blk)` declarations
/// therefore built two `DeviceRegs` windows for `device#0` at two
/// different bases, and boot handed each driver a different "authority
/// over one device instance". That is a wrong answer about which bytes a
/// device is, not a missing feature.
///
/// Fail-fast in construction order: the second binding that names an
/// already-bound device is the one that reports, naming both drivers and
/// the device.
pub fn check_device_bound_once(graph: &ImageGraph) -> Result<(), SemaError> {
    let mut first: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, d) in graph.drivers.iter().enumerate() {
        let Some(Value::ImageDecl(ImageDeclRef::Device(idx))) = d
            .args
            .iter()
            .find(|a| a.label == "device")
            .map(|a| &a.value)
        else {
            continue;
        };
        if let Some(prior) = first.get(idx) {
            return Err(build_error(format!(
                "`driver#{i}` binds device#{idx}, but `driver#{prior}` already binds that same \
                 device — 03-hardware.md §1: the device itself is named once at the image \
                 binding, the single source of truth. A second binding would mint a second \
                 `DeviceCap` over a second register window for one device"
            )));
        }
        first.insert(*idx, i);
    }
    Ok(())
}

// ===========================================================================
// plans/M7.md item G, decision 18: 03-hardware.md §7 — DriverMode is a
// const generic that changes the ISR/vector graph. Poll + vector, or Irq
// without a vector, is a sealed-graph contradiction.
// ===========================================================================
fn check_driver_mode(graph: &ImageGraph) -> Result<(), SemaError> {
    for (di, decl) in graph.drivers.iter().enumerate() {
        let Type::Named(_, targs) = &decl.actor_type else {
            continue;
        };
        let mode = targs.iter().find_map(|a| match a {
            crate::sema::types::TypeArg::Const(e) => match e {
                crate::syntax::ast::Expr::Field(base, _, variant)
                    if matches!(base.as_ref(), crate::syntax::ast::Expr::Name(_, n) if n == "DriverMode") =>
                {
                    Some(variant.as_str())
                }
                _ => None,
            },
            _ => None,
        });
        let Some(mode) = mode else {
            continue;
        };
        let vector = device_index_of_driver(decl)
            .and_then(|i| graph.devices.get(i).and_then(|d| device_vector(&d.args)));
        match (mode, vector) {
            ("Poll", Some(v)) => {
                return Err(build_error(format!(
                    "`driver#{di}` is `{}` but its device declares `vector={v}` — \
                     03-hardware.md §7: a poll build eliminates the ISR and vector entirely",
                    crate::sema::types::render_type(&decl.actor_type)
                )));
            }
            ("Irq", None) => {
                return Err(build_error(format!(
                    "`driver#{di}` is `{}` but its device declared no `vector=` — \
                     03-hardware.md §7: an IRQ build needs a vector to bind",
                    crate::sema::types::render_type(&decl.actor_type)
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Image-declared virtio-blk capacity, if any device carries
/// `capacity_sectors=`. Used by the lowerer (via `TypedProgram`) and the
/// report emitter.
pub fn blk_capacity_sectors(graph: &ImageGraph) -> Option<u64> {
    for d in &graph.devices {
        if let Some(a) = d.args.iter().find(|a| a.label == "capacity_sectors") {
            return int_value_as_i128(&a.value).and_then(|v| u64::try_from(v).ok());
        }
    }
    None
}

/// Accepted feature mask for the image's declared `required_features`,
/// already validated by `check_blk_device_decls`. Defaults to
/// `F_VERSION_1` alone when no `required_features=` is declared (the
/// mandatory bit is always present).
pub fn blk_accepted_features(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<u64, SemaError> {
    let mut enum_variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in programs.values() {
        for (name, vs) in &p.enums {
            enum_variants
                .entry(name.clone())
                .or_insert_with(|| vs.variants.clone());
        }
    }
    for d in &graph.devices {
        if let Some(a) = d.args.iter().find(|a| a.label == "required_features") {
            let names = feature_names_from_arg(a, &enum_variants)?;
            return crate::virtqueue::accepted_features(
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .map_err(build_error);
        }
    }
    Ok(crate::virtqueue::F_VERSION_1)
}

/// plans/M7.md item E1 / decision 14: every `img.device`'s
/// `required_features=` must be offerable by `virtqueue::DEVICE_FEATURES`,
/// and `capacity_sectors=` (when present) must be a positive integer
/// under the VMM's disk ceiling.
fn check_blk_device_decls(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    // Variant names from every program's enums, for Feature/VirtioFeature.
    let mut enum_variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in programs.values() {
        for (name, vs) in &p.enums {
            enum_variants
                .entry(name.clone())
                .or_insert_with(|| vs.variants.clone());
        }
    }
    for (di, d) in graph.devices.iter().enumerate() {
        if let Some(a) = d.args.iter().find(|a| a.label == "capacity_sectors") {
            let Some(v) = int_value_as_i128(&a.value) else {
                return Err(build_error(format!(
                    "`img.device` device#{di} passes a `capacity_sectors=` that is not an integer"
                )));
            };
            if v <= 0 {
                return Err(build_error(format!(
                    "`img.device` device#{di} declares `capacity_sectors={v}`; a block device \
                     needs a positive sector count"
                )));
            }
            let sectors = u64::try_from(v).map_err(|_| {
                build_error(format!(
                    "`img.device` device#{di} declares `capacity_sectors={v}` that does not fit \
                     a `u64`"
                ))
            })?;
            let bytes = sectors.saturating_mul(512);
            // Match the VMM's own ceiling so a build that would fail at
            // `BlkDevice::new` fails here instead, with the same number.
            const MAX_DISK_BYTES: u64 = 64 << 20;
            if bytes > MAX_DISK_BYTES {
                return Err(build_error(format!(
                    "`img.device` device#{di} declares `capacity_sectors={sectors}` ({bytes} bytes), \
                     which exceeds the VMM's {MAX_DISK_BYTES}-byte in-memory disk ceiling"
                )));
            }
        }
        if let Some(a) = d.args.iter().find(|a| a.label == "required_features") {
            let names = feature_names_from_arg(a, &enum_variants)?;
            crate::virtqueue::accepted_features(
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .map_err(build_error)?;
        }
    }
    Ok(())
}

/// plans/M8.md item P, decision 25 + item H attack 3: **blk configuration
/// belongs to the blk device**, and an image may carry at most one device's
/// worth of it.
///
/// Until item P an image could declare at most one pool-bearing device, so
/// "the device that declares `capacity_sectors=`" and "the device whose
/// pool hosts the ring" were the same device by construction, and
/// `blk_capacity_sectors`/`blk_accepted_features` could scan the device
/// list in declaration order. With two devices legal they are different
/// questions — and the answer to the second is only reachable through the
/// image's single `VirtQueue.configure` site, which the lowerer (reading
/// `TypedProgram::blk_capacity_sectors`) does not have in scope.
///
/// Rather than grow a second, device-scoped derivation and let the two
/// disagree, this refuses the only shape in which they could: blk
/// configuration on a device that is not the blk device. 06-machine.md §6's
/// device set is closed and machine v1 has exactly one `blk`, so the
/// report's own `BlkDevice device=device#N` line can carry only that
/// device's configuration.
///
/// When no configure site names the blk device, the "wrong device" arm
/// cannot run — but two devices both declaring `capacity_sectors=` /
/// `required_features=` is still an image carrying two devices' worth of
/// blk configuration. The graph-wide scanners would pick the first in
/// declaration order and silently drop the rest (`blk_capacity_sectors`
/// returns on the first hit; `derive_blk_report` emits no `BlkDevice` line
/// at all without a configure, so the ambiguity is invisible in the
/// report). Item H attack 3 closes that: refuse by name regardless of
/// whether anything configures a queue. One device declaring unused blk
/// config with no configure remains legal — unambiguous, nothing
/// consumes it.
fn check_blk_config_names_the_blk_device(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
    backings: &BTreeMap<String, PoolBacking>,
) -> Result<(), SemaError> {
    let mut configured: Option<&str> = None;
    for p in programs.values() {
        for (pool_name, _depth) in &p.virtqueue_configures {
            if configured.is_some() {
                // `layout::find_virtqueue_configure` owns this rejection
                // (machine v1 has exactly one queue); nothing to say here.
                // Verified by building an image with two configure sites:
                // layout refuses with its own wording
                // ("more than one `VirtQueue.configure` call; machine v1's
                // `blk` has exactly one queue").
                return Ok(());
            }
            configured = Some(pool_name.as_str());
        }
    }
    let Some(pool_name) = configured else {
        // No configure site: nothing names the blk device, so the arm
        // below cannot run. But two devices both declaring blk config is
        // still two devices' worth of configuration for a machine with
        // exactly one `blk` — refuse by name (item H attack 3). A single
        // device declaring unused capacity/features stays legal.
        let mut config_devices: Vec<usize> = Vec::new();
        for (di, d) in graph.devices.iter().enumerate() {
            if d.args
                .iter()
                .any(|a| a.label == "capacity_sectors" || a.label == "required_features")
            {
                config_devices.push(di);
            }
        }
        if config_devices.len() > 1 {
            let a = config_devices[0];
            let b = config_devices[1];
            return Err(build_error(format!(
                "`img.device` device#{a} and device#{b} both declare \
                 `capacity_sectors=`/`required_features=`, but this image has no \
                 `VirtQueue.configure` site to name the blk device — 06-machine.md §6's \
                 device set is closed and machine v1 has exactly one `blk`, so an image \
                 may carry at most one device's worth of blk configuration. Drop the \
                 declaration from every device but one"
            )));
        }
        return Ok(());
    };
    let Some(blk_device) = backings.get(pool_name).and_then(|b| b.device) else {
        // An unplaced or non-device-reachable configure pool is
        // `derive_blk_report`'s own rejection, with its own wording.
        return Ok(());
    };
    for (di, d) in graph.devices.iter().enumerate() {
        if di == blk_device {
            continue;
        }
        for label in ["capacity_sectors", "required_features"] {
            if d.args.iter().any(|a| a.label == label) {
                return Err(build_error(format!(
                    "`img.device` device#{di} declares `{label}=`, but this image's virtio-blk \
                     queue lives in pool `{pool_name}`, which is bound to device#{blk_device} — \
                     06-machine.md §6's device set is closed and machine v1 has exactly one \
                     `blk`, so the report's own `BlkDevice device=device#{blk_device}` line can \
                     carry only that device's configuration. Move the declaration to \
                     device#{blk_device}, or drop it"
                )));
            }
        }
    }
    Ok(())
}

fn feature_names_from_arg(
    a: &DeclArg,
    enum_variants: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, SemaError> {
    let Type::Array(elem, _) = &a.ty else {
        return Err(build_error(format!(
            "`required_features=` must be an array of feature variants; found `{}`",
            types::render_type(&a.ty)
        )));
    };
    let Type::Named(enum_name, _) = &**elem else {
        return Err(build_error(
            "`required_features=` elements must be feature-enum variants".to_string(),
        ));
    };
    let Some(variants) = enum_variants.get(enum_name) else {
        return Err(build_error(format!(
            "`required_features=` names enum `{enum_name}`, which has no variants in this image"
        )));
    };
    let Value::Array(items) = &a.value else {
        return Err(build_error(
            "`required_features=` must be an array value".to_string(),
        ));
    };
    let mut names = Vec::new();
    for it in items {
        let Value::Enum(idx, _) = it else {
            return Err(build_error(
                "`required_features=` elements must be enum variants".to_string(),
            ));
        };
        let Some(name) = variants.get(*idx) else {
            return Err(build_error(format!(
                "`required_features=` names variant index {idx} of `{enum_name}`, which has only \
                 {} variant(s)",
                variants.len()
            )));
        };
        names.push(name.clone());
    }
    Ok(names)
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

// --- check 3b: pool declarations (plans/M7.md item D, 05-library.md §9,
// 03-hardware.md §3 — image.graph.dma-pools, hardware.dma.pool-declared) ---
//
// 05-library.md §9's two pool intrinsics, in full:
//
//   `img.pool[T](name=P, slots=N, max_payload=B)` and
//   `img.dma_pool[T](name=P, device=d, count=N)` — bind the previously
//   unbound pool name `P` exactly once, reserve exact backing, and create
//   the initial handles; the DMA form requires a `@layout(dma)` `T` and
//   device reachability.
//
// Binding is `ImageGraph::bind_pool_name`'s (one name space, both forms);
// "create the initial handles" is the handle value each recorder already
// returns. *This* check owns the other two halves — "reserve exact
// backing" (which needs an exact size, which needs an exact per-slot size)
// and "the DMA form requires a `@layout(dma)` `T` and device
// reachability".
//
// It produces `PoolBacking`, and that one value is what the whole rest of
// the compiler reads: `layout::layout_program` places the `pooldata`
// section from it, and `layout::render_layout_section` reports it
// (03 §3's "declared ... with size, purpose, device reachability,
// alignment, and coherency policy"). One derivation, one place — a second
// copy in the placer could disagree with the checked one, and disagreeing
// about *which bytes a device can reach* is the exact failure mode
// plans/M7.md decision 5 calls a security property.
//
// **Where 03 §3's five declared facts actually come from**, honestly, one
// at a time — because 05 §9's own intrinsic surface spells only three
// arguments and inventing the other two would be worse than deriving them:
//
// - **size**: derived, exactly. `count × sizeof(T)` for the DMA form,
//   where `sizeof(T)` is item B's own exact-bytes layout — no padding, no
//   round-up, no target-dependent field (`hardware.layout.exact-bytes`).
//   `slots × max_payload` for the plain form, where both are declared.
// - **purpose**: the form itself. `img.dma_pool` declares device-reachable
//   transfer memory; `img.pool` declares CPU-side pool memory no device
//   can reach. There is no third purpose in 05 §9's surface, so this is a
//   two-valued fact and is reported as one (`kind=dma`/`kind=image`),
//   never as a free-text field nothing supplies.
// - **device reachability**: the DMA form's own `device=`. Two spellings
//   are accepted, because the normative surface uses both — 05 §9 writes
//   `device=d` naming a declared device, and 03 §3's own worked example
//   (the `ast-virtio` corpus case) writes `device=disk` naming the
//   *driver* bound to that device. A driver names exactly one device (its
//   own `device=`, 03 §1's "single source of truth"), so following that
//   one hop is a resolution, not a guess. Anything else is rejected by
//   name: a pool that is "device-reachable" from something that is not a
//   device is the one thing this check exists to prevent.
// - **alignment**: derived from the payload's own layout — the widest
//   scalar field it declares (`layout_alignment`, below), which is exactly
//   the alignment item B already *enforces* on every explicit `@offset`.
//   Deriving it from the same rule that enforces it means the two cannot
//   disagree; declaring it separately in the image would let them.
// - **coherency policy**: one value, `coherent`, and it is a *machine*
//   fact rather than a per-pool one at M7: plans/M7.md decision 5 records
//   that the flagship has no IOMMU and pools are host-mapped directly, so
//   there is exactly one policy the machine implements and no source
//   surface anywhere declares a second. Reported (it is a normative fact
//   the report must carry) but not selectable (nothing can select it).
//   The day a target offers a non-coherent pool, this is the field that
//   grows an argument, and the report already has the slot for it.

/// The largest total backing this compiler reserves for all of an image's
/// pools together. Guest DRAM is 1 GiB (06-machine.md §2) and pool backing
/// is *zeroed image bytes* like `rtdata`, so an image declaring
/// `count=2^40` must fail closed by name rather than attempt the
/// allocation — the identical reasoning (and the identical shape) as the
/// VMM's own `devices::MAX_DISK_BYTES`. 16 MiB is far past anything M7
/// needs and small enough that the failure is unmistakable.
pub const MAX_POOL_BYTES: u64 = 16 << 20;

/// One bound pool, fully resolved: everything 03-hardware.md §3 says a
/// DMA pool is declared with, plus the placement inputs. Produced once by
/// `pool_backings` and read by the checker, the placer and the report
/// alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolBacking {
    pub name: String,
    /// `true` for `img.dma_pool` (device-reachable transfer memory),
    /// `false` for `img.pool` — 03 §3's "purpose", as the two-valued fact
    /// 05 §9's surface actually offers.
    pub is_dma: bool,
    /// The payload type as source spelled it (`types::render_type`).
    pub payload: String,
    /// `count=` (DMA) or `slots=` (plain).
    pub slots: u64,
    /// The exact bytes one slot occupies: item B's own `@layout(dma)`
    /// size for the DMA form, the declared `max_payload=` for the plain
    /// form.
    pub slot_bytes: u64,
    /// `slots * slot_bytes`, checked.
    pub bytes: u64,
    /// The payload's own natural alignment (`layout_alignment`), or 8 for
    /// a plain pool whose payload has no `@layout` at all.
    pub align: u64,
    /// The declared device this pool is reachable from — `None` for a
    /// plain `img.pool`, which no device can reach.
    pub device: Option<usize>,
}

/// A `@layout` type's own natural alignment: the widest scalar field it
/// declares, clamped to a power of two in `1..=8`. This is not a new rule
/// — `types::check_one_layout` already *requires* every explicit
/// `@offset(n)` to be `size`-byte aligned for its own field, so the
/// widest field's size is exactly the alignment the whole type has to be
/// placed at for every one of its fields to land aligned. A layout with
/// no fields at all (unrepresentable: `check_one_layout` rejects a
/// zero-size layout) would answer 1.
pub fn layout_alignment(l: &types::LayoutType) -> u64 {
    let widest = l
        .entries
        .iter()
        .filter_map(|e| match e {
            types::LayoutEntry::Field(f) => Some(f.size),
            types::LayoutEntry::Padding { .. } => None,
        })
        .max()
        .unwrap_or(1);
    // A `Bytes[N]` field is N bytes wide but needs no more than byte
    // alignment; clamping at 8 keeps this the machine's own widest scalar
    // alignment rather than an arbitrary array's length.
    match widest {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        _ => 8,
    }
}

/// Every `@layout` type in the build closure, by name — the union of every
/// module's own already-checked table (`TypedProgram::layouts`).
///
/// **A same-spelling collision across modules is refused** (plans/M7.md
/// item I's sweep). This function used to resolve one last-module-wins in
/// `BTreeMap` order, disclosed as "the identical, already-recorded
/// simplification `layout::merge_layout_ctx` makes". That framing
/// understated it, because *this* table is what
/// `img.dma_pool[T](name=P, ...)` reads to answer how many bytes a device
/// can reach. Verified by running: module `a.main` declaring
/// `@layout(dma) struct Ctl: tiny: u8` and binding
/// `img.dma_pool[Ctl](name=Control, count=4)` against its own `Ctl`, plus
/// an imported module `z.other` declaring an unrelated 24-byte `Ctl`,
/// silently reserved 4 x 24 bytes — and with `z.other`'s `Ctl` spelled
/// `@layout(mmio)` the build was *rejected* for a kind `a.main`'s own
/// `Ctl` does not have and cannot see. A wrong answer about device-
/// reachable bytes in the first case and an unactionable diagnostic in
/// the second, both from a type name the author never wrote.
///
/// A real fix is cross-module type identity, which this compiler does not
/// have anywhere (`layout::merge_layout_ctx` merges every struct and enum
/// name the same way, and no type name resolves across a module boundary
/// at all today, so nothing can even *name* the other module's type).
/// Until it does, two `@layout` types with one name in one closure is a
/// build error rather than a coin flip.
///
/// `pub(crate)` so `layout::layout_program` can hand `pool_backings` the
/// same table this pass does. It builds its own from the raw `ast::Module`
/// closure instead (`types::check_layouts` is a pure function of one
/// specialized module, so the two cannot disagree) — the identical
/// arrangement `bin/wrela.rs` already uses to render the report's own
/// exact-bytes section.
pub(crate) fn closure_layouts(
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<BTreeMap<String, types::LayoutType>, SemaError> {
    let mut out: BTreeMap<String, types::LayoutType> = BTreeMap::new();
    let mut from: BTreeMap<String, String> = BTreeMap::new();
    for (module, p) in programs {
        for l in &p.layouts {
            if let Some(prior) = from.get(&l.name) {
                return Err(build_error(format!(
                    "two modules in this build closure declare a `@layout` type named \
                     `{}` — `{prior}` and `{module}`. A `@layout` type's identity is its \
                     bare name here (no type name resolves across a module boundary in this \
                     compiler), so a pool or a driver naming `{}` would silently get whichever \
                     one this table happened to keep, and 03-hardware.md §3's exact bytes are \
                     the bytes a device reads. Rename one of them",
                    l.name, l.name
                )));
            }
            from.insert(l.name.clone(), module.clone());
            out.insert(l.name.clone(), l.clone());
        }
    }
    Ok(out)
}

fn pool_arg<'a>(args: &'a [DeclArg], label: &str) -> Option<&'a DeclArg> {
    args.iter().find(|a| a.label == label)
}

/// A pool argument that must be a positive count that fits a `u64`.
fn pool_count(name: &str, spelling: &str, args: &[DeclArg], label: &str) -> Result<u64, SemaError> {
    let Some(a) = pool_arg(args, label) else {
        return Err(build_error(format!(
            "`{spelling}(name={name}, ...)` declares no `{label}=` — 05-library.md §9's pool \
             intrinsics reserve *exact* backing, and this compiler never guesses a capacity"
        )));
    };
    let Some(v) = int_value_as_i128(&a.value) else {
        return Err(build_error(format!(
            "`{spelling}(name={name}, ...)` passes a `{label}=` that is not an integer — a pool's \
             backing is a build-time fact (02-language.md §4)"
        )));
    };
    if v <= 0 {
        return Err(build_error(format!(
            "`{spelling}(name={name}, {label}={v})` — a pool reserves at least one slot; a \
             zero-or-negative `{label}=` reserves nothing and would leave every handle \
             unallocatable"
        )));
    }
    u64::try_from(v).map_err(|_| {
        build_error(format!(
            "`{spelling}(name={name}, {label}={v})` does not fit a `u64`"
        ))
    })
}

/// Rejects any labeled argument the intrinsic does not define. 05 §9
/// spells each pool form's arguments exactly; an unrecognized one is
/// either a typo for a real one or a fact this compiler would silently
/// drop, and both are build errors rather than accepted noise.
fn pool_labels(
    name: &str,
    spelling: &str,
    args: &[DeclArg],
    allowed: &[&str],
) -> Result<(), SemaError> {
    if let Some(a) = args.iter().find(|a| !allowed.contains(&a.label.as_str())) {
        return Err(build_error(format!(
            "`{spelling}(name={name}, ...)` has no `{}=` argument (05-library.md §9 spells it \
             `{spelling}[T](name=P, {})`)",
            a.label,
            allowed
                .iter()
                .filter(|l| **l != "name")
                .map(|l| format!("{l}=..."))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

/// The DMA form's own `device=`, resolved to a declared device index.
/// Both normative spellings are accepted (this section's own doc comment):
/// a declared device directly, or the driver bound to it.
fn pool_device(name: &str, args: &[DeclArg], graph: &ImageGraph) -> Result<usize, SemaError> {
    let Some(a) = pool_arg(args, "device") else {
        return Err(build_error(format!(
            "`img.dma_pool(name={name}, ...)` declares no `device=` — 03-hardware.md §3: all \
             memory a device can reach originates from its bound pools, so a DMA pool with no \
             device is memory nothing can reach"
        )));
    };
    let idx = match &a.value {
        Value::ImageDecl(ImageDeclRef::Device(i)) => *i,
        Value::ImageDecl(ImageDeclRef::Driver(i)) => {
            // 03 §1: the device is named once, at the image binding, so a
            // driver names exactly one device. Following that one hop is
            // a resolution, not a guess.
            let Some(d) = graph.drivers.get(*i) else {
                return Err(build_error(format!(
                    "`img.dma_pool(name={name}, device=driver#{i})` names a driver this image \
                     does not declare"
                )));
            };
            match pool_arg(&d.args, "device").map(|x| &x.value) {
                Some(Value::ImageDecl(ImageDeclRef::Device(j))) => *j,
                _ => {
                    return Err(build_error(format!(
                        "`img.dma_pool(name={name}, device=driver#{i})` names a driver that binds \
                         no declared device of its own, so this pool is reachable from nothing \
                         (03-hardware.md §1: the device is named once, at the image binding)"
                    )));
                }
            }
        }
        Value::ImageDecl(r) => {
            return Err(build_error(format!(
                "`img.dma_pool(name={name}, device={})` must name a declared device (or the \
                 driver bound to one) — 03-hardware.md §3: all memory a device can reach \
                 originates from its bound pools",
                r.render()
            )));
        }
        _ => {
            return Err(build_error(format!(
                "`img.dma_pool(name={name}, device=...)` must name a declared device (or the \
                 driver bound to one) — 03-hardware.md §3: all memory a device can reach \
                 originates from its bound pools"
            )));
        }
    };
    if graph.devices.get(idx).is_none() {
        return Err(build_error(format!(
            "`img.dma_pool(name={name}, device=device#{idx})` names a device this image does not \
             declare"
        )));
    }
    Ok(idx)
}

/// Every bound pool, resolved and checked, keyed by name — one
/// deterministic order (`BTreeMap`, so `image.report.deterministic` holds
/// by construction) that does not depend on which *form* bound the name.
pub fn pool_backings(
    graph: &ImageGraph,
    layouts: &BTreeMap<String, types::LayoutType>,
) -> Result<BTreeMap<String, PoolBacking>, SemaError> {
    let mut out: BTreeMap<String, PoolBacking> = BTreeMap::new();

    for (name, d) in &graph.pools {
        pool_labels(name, "img.pool", &d.args, &["name", "slots", "max_payload"])?;
        let slots = pool_count(name, "img.pool", &d.args, "slots")?;
        let slot_bytes = pool_count(name, "img.pool", &d.args, "max_payload")?;
        out.insert(
            name.clone(),
            PoolBacking {
                name: name.clone(),
                is_dma: false,
                payload: types::render_type(&d.payload_type),
                slots,
                slot_bytes,
                bytes: 0, // filled below, once
                align: 8,
                device: None,
            },
        );
        let b = out.get_mut(name).expect("just inserted");
        b.bytes = slots.checked_mul(slot_bytes).ok_or_else(|| {
            build_error(format!(
                "`img.pool(name={name}, slots={slots}, max_payload={slot_bytes})` reserves more \
                 than a `u64` of backing"
            ))
        })?;
    }

    for (name, d) in &graph.dma_pools {
        pool_labels(name, "img.dma_pool", &d.args, &["name", "device", "count"])?;
        let payload = types::render_type(&d.payload_type);
        // 03-hardware.md §3: "**Transfer payloads** are `own[P] T` where
        // `P` is a device-bound DMA pool and `T` is `@layout(dma)`."
        let Some(l) = layouts.get(&payload) else {
            return Err(build_error(format!(
                "`img.dma_pool[{payload}](name={name}, ...)` — `{payload}` is not a `@layout(dma)` \
                 type in this build closure. 03-hardware.md §3: a transfer payload's `T` is \
                 `@layout(dma)`, so the compiler can report its exact size, offsets, padding and \
                 endianness before a device ever reads it"
            )));
        };
        if l.kind != types::LayoutKind::Dma {
            return Err(build_error(format!(
                "`img.dma_pool[{payload}](name={name}, ...)` — `{payload}` is `@layout({})`, not \
                 `@layout(dma)`. 03-hardware.md §3 gives each kind its own meaning: `dma` is \
                 device-visible memory checked against the target ABI, `mmio` is a register map, \
                 `wire` is target-independent persistent bytes, `runtime` is the machine's own \
                 tables (§3.1)",
                l.kind.as_str()
            )));
        }
        let device = pool_device(name, &d.args, graph)?;
        let slots = pool_count(name, "img.dma_pool", &d.args, "count")?;
        // plans/M10.md item A2b: a layout whose sizing is still deferred has
        // no byte count, and a pool's backing is exactly a byte count times a
        // slot count — so this asks for the size by name and fails closed if
        // there is none, rather than reading a 0 and reserving nothing. (A
        // `dma` layout cannot defer today: only a `runtime` layout may have
        // an array field at all, and this arm already refused every other
        // kind above. The guard is here because "cannot" is a property of
        // today's rules, not of this call site.)
        let slot_bytes = l.require_size(&format!("`img.dma_pool[{payload}]`'s backing"))?;
        let bytes = slots.checked_mul(slot_bytes).ok_or_else(|| {
            build_error(format!(
                "`img.dma_pool(name={name}, count={slots})` of a {slot_bytes}-byte `{payload}` \
                 reserves more than a `u64` of backing"
            ))
        })?;
        out.insert(
            name.clone(),
            PoolBacking {
                name: name.clone(),
                is_dma: true,
                payload,
                slots,
                slot_bytes,
                bytes,
                align: layout_alignment(l),
                device: Some(device),
            },
        );
    }

    // **The two M7 post-closure guards that used to stand here are gone
    // (plans/M8.md item P, decision 24).** They refused two pool-bearing
    // devices, and a pool bound to a driverless device when some other
    // device was driven — not because either shape is illegal, but
    // because the `BlkPool name= base= size=` mapping line carried no
    // device and `wrela-vmm`'s `parse_report` handed every window to its
    // one device model. That was a fail-open in the *artifact*, held shut
    // by refusing images the language allows.
    //
    // The line now carries `device=device#N` (`layout::render_layout_section`
    // / `append_blk_vmm_lines`), `devices::GuestMem` carries the device
    // whose view it is, and its `window_offset` — still the single
    // guest→host conversion site — admits a range only if it lies inside a
    // window bound to *that* device. 03-hardware.md §3's "all memory a
    // device can reach originates from *its* bound pools" is therefore
    // enforced where reaching happens, and the oracle is a boot:
    // `golden/err-boot-blk-cross-device-pool` hands the blk driver a
    // payload from another device's pool and the model refuses the
    // descriptor by name.
    //
    // What 03 §1 still refuses, unchanged and one pass later, is the
    // *capability*: `check_dma_pool_mint` rejects a driver bound to
    // device#Y taking `DmaPool[P, N]` for a pool declared reachable from
    // device#X (`golden/err-dma-pool-mint-wrong-device`). That is the
    // mint-time rule, at the binding §1 puts it at; it is not a
    // restatement of the reachability rule, and neither implies the other.

    let total: u64 = out.values().map(|b| b.bytes).fold(0, u64::saturating_add);
    if total > MAX_POOL_BYTES {
        return Err(build_error(format!(
            "this image's pools reserve {total} bytes of backing, past the {MAX_POOL_BYTES}-byte \
             ceiling this compiler will place — pool backing is zeroed image bytes (like the \
             actor runtime tables), and guest DRAM is 1 GiB (06-machine.md §2). Failing closed \
             rather than emitting an image no machine can load"
        )));
    }
    Ok(out)
}

/// Every `own[P] T` a type names, at any nesting, as `(pool name, payload
/// type)` pairs.
fn own_handles_in_type(ty: &Type, out: &mut Vec<(String, Type)>) {
    use crate::sema::types::TypeArg;
    match ty {
        Type::Own(pool, inner) => {
            out.push((pool.clone(), (**inner).clone()));
            own_handles_in_type(inner, out);
        }
        Type::Static(inner) | Type::Option(inner) => own_handles_in_type(inner, out),
        Type::Array(elem, _) => own_handles_in_type(elem, out),
        Type::Tuple(elems) => {
            for e in elems {
                own_handles_in_type(e, out);
            }
        }
        Type::Result(ok, err) => {
            own_handles_in_type(ok, out);
            own_handles_in_type(err, out);
        }
        Type::Fn(params, ret) => {
            for (_, t) in params {
                own_handles_in_type(t, out);
            }
            own_handles_in_type(ret, out);
        }
        Type::Named(_, targs) => {
            for a in targs {
                if let TypeArg::Type(t) = a {
                    own_handles_in_type(t, out);
                }
            }
        }
        _ => {}
    }
}

fn own_handles_in_fn(f: &crate::sema::typed::TypedFn, out: &mut Vec<(String, Type)>) {
    for p in &f.params {
        own_handles_in_type(&p.ty, out);
    }
    own_handles_in_type(&f.ret, out);
}

fn own_handles_in_struct(s: &crate::sema::typed::TypedStruct, out: &mut Vec<(String, Type)>) {
    for t in s.field_types.values() {
        own_handles_in_type(t, out);
    }
    for f in s.methods.values() {
        own_handles_in_fn(f, out);
    }
    for f in s.assoc_fns.values() {
        own_handles_in_fn(f, out);
    }
    if let Some(f) = &s.init {
        own_handles_in_fn(f, out);
    }
}

/// Every `own[P] T` spelled anywhere in the build closure's *declaration*
/// surface: const types, fn signatures, struct fields, and every
/// `init`/method/assoc fn of a struct or a generic instantiation.
///
/// Deliberately not a body walk, and that is exact rather than a hedge:
/// nothing in this language constructs an `own[P] T` — no literal, no
/// cast, no intrinsic (`hardware.dma.ownership-transfer`'s own
/// unforgeability half) — so every handle a body can hold arrived through
/// one of the declarations walked here. A local's type is a consequence of
/// this surface, never an independent source of one.
fn own_handles_in_closure(programs: &BTreeMap<String, TypedProgram>) -> Vec<(String, Type)> {
    use crate::sema::typed::TypedInstantiation;
    let mut out = Vec::new();
    for p in programs.values() {
        for c in p.consts.values() {
            own_handles_in_type(&c.ty, &mut out);
        }
        for f in p.fns.values() {
            own_handles_in_fn(f, &mut out);
        }
        for s in p.structs.values() {
            own_handles_in_struct(s, &mut out);
        }
        for inst in p.instantiations.values() {
            match inst {
                TypedInstantiation::Fn(f) => own_handles_in_fn(f, &mut out),
                TypedInstantiation::Struct(s) => own_handles_in_struct(s, &mut out),
                TypedInstantiation::Enum => {}
            }
        }
    }
    out
}

/// 05-library.md §9 + 03-hardware.md §3, checked (plans/M7.md item D).
/// Resolves every bound pool (`pool_backings`, which is where every
/// per-declaration rejection lives) and then enforces the one rule that
/// spans the pool and the code that names it: an `own[P] T` handle's `T`
/// is the payload type `P` was bound with.
pub fn check_pool_decls(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    let backings = pool_backings(graph, &closure_layouts(programs)?)?;
    // 02-language.md §4: "`own[Name] T` is a movable, uniquely owned
    // handle to a `T` allocated from that pool" — one pool node, one
    // payload type. For a DMA pool that sentence is also 03 §3's
    // `@layout(dma)` requirement, since `pool_backings` already proved the
    // pool's own payload type is one.
    for (pool, payload) in own_handles_in_closure(programs) {
        let Some(b) = backings.get(&pool) else {
            continue; // not bound by this image: `check_pools_bound` owns that.
        };
        let spelled = types::render_type(&payload);
        if spelled == b.payload {
            continue;
        }
        let why = if b.is_dma {
            format!(
                " — and this is a DMA pool, whose slot is exactly the {} bytes of `{}` \
                 (03-hardware.md §3: a transfer payload's `T` is `@layout(dma)`, reported to the \
                 byte), so an `own[{pool}] {spelled}` handle would name a slot of the wrong shape \
                 for a device to read or write",
                b.slot_bytes, b.payload
            )
        } else {
            String::new()
        };
        return Err(build_error(format!(
            "`own[{pool}] {spelled}` names pool `{pool}`, which this image binds with payload type \
             `{}` (02-language.md §4: a pool name is bound to exactly one pool node, and \
             `own[P] T` is a handle to a `T` allocated from *that* pool){why}",
            b.payload
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
        // plans/M8.md item D: `mailbox=` is image wiring on *both* forms.
        // For `img.actor` it is required (M6 decision 3); for `img.driver`
        // it is what makes the driver messageable at all (05-library.md
        // §9), and its absence is the floor `err-image-driver-message`
        // pins.
        DeclKind::Driver => &["device", "core", "mailbox"],
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

/// 03-hardware.md §1's own capability list, with each name's fixed
/// generic arity: `DeviceCap[D]`, `Mmio[L]`, `IrqCap[V]`, `DmaPool[P, N]`.
/// One list, in one place, deliberately (plans/M7.md item A): it is read
/// by `layout::build_boot_init_calls` (which must fail closed on exactly
/// the parameters this pass accepts by substitution), by
/// `sema::symbols::is_resolvable_without_import` (so the names are in
/// scope with no import), by `sema::types::resolve_named` (which mints
/// the type), and by
/// `sema::types::check_layouts` (03 §3 forbids a capability inside a
/// layout). Several copies could disagree; one cannot.
///
/// 03 §1's fifth bullet — "target-specific narrow capabilities (queue
/// notifiers, ...)" — is deliberately absent: no target-specific
/// capability is named by any normative rule yet, and inventing one here
/// would be a list entry with nothing behind it. The day one exists it is
/// added here and every consumer above picks it up unchanged.
///
/// **`DmaShared[P, L]` is on this list and is not one of §1's four**
/// (plans/M7.md item D). It is 03-hardware.md §3's *shared control
/// memory* — "permanently shared, exposing only field-wise typed
/// operations that carry the target's volatile/cache/ordering
/// semantics. It cannot be read as bytes or lent as a plain value." It
/// belongs here because every consumer of this list asks it the same
/// question and gets the same right answer: it is unforgeable (no
/// declaration, import, construction or cast makes one); a fn holding one
/// touches DMA state, which is §1's own provenance sentence verbatim; an
/// `@actor` may not hold one and a `@driver` may; it has no byte encoding
/// and so cannot sit inside a `@layout`; and nothing mints one at image
/// binding. The one place the distinction is load-bearing —
/// `check_capability_substitution`'s diagnostic — names it separately, so
/// the list never claims §1 says something §1 does not.
const CAPABILITY_TYPES: &[(&str, usize)] = &[
    ("DeviceCap", 1),
    ("DmaPool", 2),
    ("DmaShared", 2),
    ("IrqCap", 1),
    ("Mmio", 1),
];

/// Shared with `layout::build_boot_init_calls` for the same reason
/// `is_reserved_actor_arg` above is: this pass *accepts* a parameter of
/// one of these types with no explicit argument (decision 7's own
/// substitution rule), and boot has to recognize exactly the same set to
/// fail closed on it by name. Two copies of this list could disagree; one
/// cannot.
pub(crate) fn is_capability_type_name(name: &str) -> bool {
    capability_generic_arity(name).is_some()
}

/// How many generic arguments `name` takes, if it is a capability type at
/// all — the same single list, asked a second question. `sema::types`'
/// own resolver needs the arity to reject `Mmio[A, B]` or a bare
/// `DeviceCap` by name rather than by accident.
pub(crate) fn capability_generic_arity(name: &str) -> Option<usize> {
    CAPABILITY_TYPES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, arity)| *arity)
}

/// 03-hardware.md §9's own bring-up chain, as types: "Device bring-up is
/// a typed state chain (for virtio: `Reset -> Acknowledged ->
/// DriverClaimed -> FeaturesNegotiated -> FeaturesAccepted ->
/// QueuesConfigured -> Running`)". One name per state, in chain order,
/// each taking the device type as its single argument — the spelling
/// `docs/language/examples/virtio-storage.wr` already fixes for the last
/// of them (`device: RunningDevice[VirtioBlock]`), applied uniformly to
/// the other six rather than invented per state (plans/M7.md item H1).
///
/// These are **not** 03 §1 capabilities and the list is deliberately
/// separate: a capability is authority over a device, a queue or a pool,
/// minted at the image binding; a protocol state is the sealed transport's
/// own position in the bring-up chain, produced only by a transition. The
/// two lists are asked the same *structural* questions
/// (`is_sealed_authority_type_name` below — unforgeable, a resource,
/// no byte encoding, an `@actor` may not hold one) and different
/// *diagnostic* ones, which is exactly why they are two lists read through
/// one predicate rather than one list with a footnote.
const PROTOCOL_STATE_TYPES: &[&str] = &[
    "ResetDevice",
    "AcknowledgedDevice",
    "DriverClaimedDevice",
    "FeaturesNegotiatedDevice",
    "FeaturesAcceptedDevice",
    "QueuesConfiguredDevice",
    "RunningDevice",
];

/// Is `name` one of 03 §9's seven bring-up states? Every one takes
/// exactly one type argument (the device type), so there is no arity
/// table here — the arity is the constant 1.
pub(crate) fn is_protocol_state_type_name(name: &str) -> bool {
    PROTOCOL_STATE_TYPES.contains(&name)
}

/// The union of 03 §1's capabilities and 03 §9's protocol states — every
/// type this language mints only inside the sealed transport, and never
/// from source. This is the predicate every *structural* rule asks:
/// the name cannot be declared, imported, constructed, called or cast to;
/// the type is a resource by fiat; it has no byte encoding, so no
/// `@layout` may hold one; an `@actor` may not hold one and a `@driver`
/// may; and `mwir::size_of`/`codegen::is_aggregate` carry it as one
/// opaque 8-byte word.
pub(crate) fn is_sealed_authority_type_name(name: &str) -> bool {
    is_capability_type_name(name)
        || is_protocol_state_type_name(name)
        // plans/M7.md item E1: `VirtQueue[..N]` is sealed authority over
        // one split ring (03-hardware.md §4) — unforgeable, a resource,
        // one opaque word at runtime (the control-pool base the ring
        // lives in). Not one of §1's four capabilities and not a §9
        // bring-up state, but every structural rule asks the same
        // question of it.
        || name == "VirtQueue"
        // plans/M7.md item E2: the permit `reserve_proven` yields and the
        // operation `prepare_block` yields (03-hardware.md §4) — sealed
        // resources, one opaque word each, never constructible from source.
        || name == "QueuePermit"
        || name == "QueueOp"
        // plans/M7.md item E3: `Receipt[P]` (03-hardware.md §5) — the
        // sealed resource state machine for published work. One opaque
        // word (caller endpoint); never constructible from source.
        || name == "Receipt"
}

/// The noun phrase (with its normative citation) a diagnostic uses for one
/// of those names, so a rule shared by both lists still says which of the
/// two the author actually wrote. Answers for a non-sealed name too — the
/// callers all guard with `is_sealed_authority_type_name` first, and a
/// default of "capability" would be a quiet lie if one ever did not.
pub(crate) fn sealed_authority_kind(name: &str) -> &'static str {
    if is_protocol_state_type_name(name) {
        "a sealed protocol state (03-hardware.md §9)"
    } else if name == "VirtQueue" {
        "a sealed queue (03-hardware.md §4)"
    } else if name == "QueuePermit" || name == "QueueOp" {
        "a sealed queue value (03-hardware.md §4)"
    } else if name == "Receipt" {
        "a sealed receipt (03-hardware.md §5)"
    } else {
        "a capability type (03-hardware.md §1)"
    }
}

/// 02-language.md §3.1's second bullet: protocol resources whose only
/// consumers are protocol operations — every control-flow path must
/// explicitly consume, return, or transfer them (or cover with `defer`).
/// plans/M7.md item E3 flips `values.resource.protocol-consumption`.
///
/// **In:** `DeviceCap` / `Mmio` / `IrqCap` (capability types whose only
/// consumers are protocol ops); every §9 bring-up state; `VirtQueue` /
/// `QueuePermit` / `QueueOp` / `Receipt`.
///
/// **Out — and why that is not an exception:**
/// - `DmaPool[P, N]`: §3.1's *first* bullet names "pool handles" as the
///   compiler-known non-failing reclaim case. A `DmaPool` is that handle;
///   putting it in bullet two would invent a consume-on-every-path
///   obligation the first bullet already answers.
/// - `DmaShared[P, L]`: permanently shared control memory (03 §3). Its
///   field-wise ops do not consume the handle, and the language has no
///   terminal sink for a bare `DmaShared` value (returning one is refused;
///   `VirtQueue.configure` keeps the shared memory inside the queue).
///   Forcing bullet two here would require a laundering holder or a
///   vacuous drop. When a real sink exists, add the name.
pub(crate) fn is_protocol_consuming_type_name(name: &str) -> bool {
    matches!(
        name,
        "DeviceCap" | "Mmio" | "IrqCap" | "VirtQueue" | "QueuePermit" | "QueueOp" | "Receipt"
    ) || is_protocol_state_type_name(name)
}

/// Name-only leaf of the protocol-consumption predicate. Composites and
/// plain wrapper structs are answered by `sema::flow`'s
/// `protocol_resource_carried`, which walks Option/array/tuple/`Result`/
/// named fields the same way `type_contains_capability` does — item I's
/// sweep found `Option[DeviceCap]` / `CapBundle { cap: DeviceCap }` drops
/// silently accepted when this leaf was used alone.
pub(crate) fn is_protocol_consuming_type(ty: &Type) -> bool {
    match ty {
        Type::Named(name, _) => is_protocol_consuming_type_name(name),
        _ => false,
    }
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
fn find_constructor(programs: &BTreeMap<String, TypedProgram>, actor_type: &Type) -> Constructor {
    let Type::Named(struct_name, targs) = actor_type else {
        return Constructor::Init(Vec::new());
    };
    // plans/M7.md item G, decision 18: mode-generic drivers' `init` lives
    // on the instantiation, not the unsubstituted template.
    if !targs.is_empty() {
        let key = format!("struct:{}", crate::sema::types::render_type(actor_type));
        for p in programs.values() {
            if let Some(crate::sema::typed::TypedInstantiation::Struct(s)) =
                p.instantiations.get(&key)
            {
                if let Some(f) = &s.init {
                    return Constructor::Init(
                        f.params
                            .iter()
                            .map(|p| (p.name.clone(), p.ty.clone()))
                            .collect(),
                    );
                }
                return Constructor::Fields(
                    s.fields
                        .iter()
                        .filter_map(|name| {
                            s.field_types.get(name).map(|ty| (name.clone(), ty.clone()))
                        })
                        .collect(),
                );
            }
        }
        return Constructor::Init(Vec::new());
    }
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

/// 03-hardware.md §1's minting sentence, made mechanical (plans/M7.md
/// item A): "unforgeable resource values minted while the image binds a
/// declared device to a `@driver` ... The device itself is named once, at
/// the image binding (`img.driver(BlkDriver, device=blk_device)`), the
/// single source of truth."
///
/// This is the *typed* half of the mint, and it is the whole half item A
/// ships. It decides whether a declared capability `init` parameter has a
/// real minting source in this image, and what that source is — but it
/// produces no runtime value, because there is nothing yet to produce:
/// `layout::build_boot_init_calls` walks `graph.actors` only, so a driver's
/// `init` is never called at boot at all (M6-D's own floor, unchanged
/// here). The parameter's *provenance* is checked; its *bytes* are items
/// D and E.
///
/// `DeviceCap[D]` was the only mint item A shipped, exactly §1's sentence:
/// the declaration's own `device=` argument, whose declared device type
/// must be `D`. plans/M7.md item D adds the second, `DmaPool[P, N]`, now
/// that a declared pool is real memory: `P` must be a DMA pool this image
/// binds, reachable from the very device this binding names, and `N` must
/// cover the backing that pool actually reserves. The remaining two name
/// the item that mints them and fail closed, because each needs machinery
/// that does not exist — `Mmio[L]` needs a claim to partition (item C),
/// `IrqCap[V]` needs a vector bound from the image graph (item G) — and
/// `DmaShared[P, L]` fails closed too, for a different reason worth
/// keeping distinct: 03 §3's shared control memory is not minted at the
/// image binding at all in the normative example, it is produced by
/// configuring a queue out of a pool (item E).
fn check_capability_substitution(
    decl_ref: &ImageDeclRef,
    struct_name: &str,
    param_name: &str,
    param_ty: &Type,
    cap_name: &str,
    kind: DeclKind,
    args: &[DeclArg],
    graph: &ImageGraph,
    backings: &BTreeMap<String, PoolBacking>,
    device_caps_seen: &mut usize,
) -> Result<(), SemaError> {
    let rendered = types::render_type(param_ty);
    // An `img.actor(...)` declaration has no `device=` at all (it is not
    // one of its reserved labels — a `device=` there is rejected as an
    // unknown argument), so no capability parameter of one can ever have a
    // minting source. Sema already rejects an `@actor` *declaring* such a
    // parameter (`hardware.capabilities.unforgeable`); this is the arm for
    // the shape sema cannot see, an `img.actor(...)` naming a struct that
    // is not an `@actor` at all.
    if matches!(kind, DeclKind::Actor) {
        return Err(build_error(format!(
            "`{}` wires `{struct_name}` as an actor, but `{struct_name}.init` takes `{param_name}: \
             {rendered}` — a capability is minted only where the image binds a declared device to \
             a `@driver` (03-hardware.md §1), and `img.actor(...)` binds no device",
            decl_ref.render()
        )));
    }
    if cap_name == "DmaPool" {
        return check_dma_pool_mint(decl_ref, struct_name, param_name, param_ty, args, backings);
    }
    if cap_name != "DeviceCap" {
        // plans/M7.md item H1 rewrote the `Mmio` arm, and decision 10 says
        // exactly why: this arm's old text said "nothing mints a `Mmio`
        // yet — that is plans/M7.md item C", which was already wrong when
        // it was written and is now false twice over. An `Mmio[L]` *is*
        // mintable — but never *here*. 03-hardware.md §2's own sentence is
        // "a driver **or sealed protocol** partitions its claim", and 03
        // §9's transport is what does it: `claimed.map_partition(L)`, on a
        // state the driver's own `init` obtained from `claim`. The image
        // binding mints one capability and one only, the `DeviceCap[D]`
        // the `device=` names. So this is a *permanent* rejection with a
        // redirection, not a fail-closed floor waiting on an item.
        let owner = match cap_name {
            "Mmio" => {
                "an `Mmio[L]` is not minted at the image binding at all: the sealed transport \
                 hands one out from an already-claimed device \
                 (`claimed.map_partition(L)` — 03-hardware.md §2/§9). Declare it as a `@driver` \
                 field and assign it inside `init`, which is where 03 §1's own worked \
                 constructor puts it"
            }
            "IrqCap" => {
                // plans/M7.md item G: an `IrqCap[V]` at the image binding
                // is minted from the device's own `vector=` — decision
                // 12's word is that bit index. Prefer
                // `claimed.take_irq()` inside `init` (03 §6's own
                // spelling); an `init` parameter is accepted when the
                // device declared a vector, rejected when it did not.
                return check_irq_cap_mint(
                    decl_ref,
                    struct_name,
                    param_name,
                    param_ty,
                    args,
                    graph,
                );
            }
            // 03-hardware.md §3's shared control memory. Not "no queue
            // exists" — `VirtQueue.configure` (plans/M7.md item E1) mints
            // one out of a pool — but "nothing at the image binding
            // produces one": the image binds the pool; the queue makes
            // the shared control memory.
            _ => {
                "`VirtQueue.configure(pool=take ..., ...)` mints a `DmaShared[P, L]` out of a \
                 DMA pool (plans/M7.md item E1 / 03-hardware.md §3); the image binding does not \
                 — bind the pool and configure the queue inside the driver's `init`"
            }
        };
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init` takes `{param_name}: \
             {rendered}` — {owner}. Failing closed rather than substituting an unminted capability",
            decl_ref.render()
        )));
    }
    *device_caps_seen += 1;
    if *device_caps_seen > 1 {
        return Err(build_error(format!(
            "`{}` binds one device to `{struct_name}`, but `{struct_name}.init` takes more than \
             one `DeviceCap` parameter (`{param_name}: {rendered}` is the second) — \
             03-hardware.md §1: a `DeviceCap[D]` is authority over *one* device instance, and the \
             device is named once at the image binding",
            decl_ref.render()
        )));
    }
    let Some(device_arg) = args.iter().find(|a| a.label == "device") else {
        return Err(build_error(format!(
            "`{}` declares no `device=`, but `{struct_name}.init` takes `{param_name}: {rendered}` \
             — 03-hardware.md §1: a capability is minted while the image binds a declared device \
             to a `@driver`, and the device is named once, at the image binding, the single \
             source of truth",
            decl_ref.render()
        )));
    };
    let Value::ImageDecl(ImageDeclRef::Device(idx)) = &device_arg.value else {
        return Err(build_error(format!(
            "`{}` passes a `device=` that is not a declared device, so `{struct_name}.init`'s own \
             `{param_name}: {rendered}` has nothing to be minted from (03-hardware.md §1)",
            decl_ref.render()
        )));
    };
    let Some(device) = graph.devices.get(*idx) else {
        return Err(build_error(format!(
            "`{}` passes a `device=` naming device#{idx}, which this image does not declare",
            decl_ref.render()
        )));
    };
    // `D` must be the bound device's own declared type: §1's "the single
    // source of truth" is only true if the two agree, and a `DeviceCap[A]`
    // minted from a device declared `img.device[B](...)` would be
    // authority over a device this image never bound.
    let bound = types::render_type(&device.device_type);
    let declared = match param_ty {
        Type::Named(_, targs) => match targs.first() {
            Some(crate::sema::types::TypeArg::Type(t)) => types::render_type(t),
            _ => bound.clone(), // arity is guaranteed by `sema::types`
        },
        _ => bound.clone(),
    };
    if declared != bound {
        return Err(build_error(format!(
            "`{}` binds device#{idx} (declared `{bound}`) to `{struct_name}`, but \
             `{struct_name}.init` takes `{param_name}: {rendered}` — a `DeviceCap[D]` is minted \
             from the device this binding names, and `{declared}` is not `{bound}` \
             (03-hardware.md §1)",
            decl_ref.render()
        )));
    }
    Ok(())
}

/// 03-hardware.md §1's second mint (plans/M7.md item D):
/// `take pool: DmaPool[BlockControl, 256.KiB]`, the other half of §1's own
/// worked driver constructor.
///
/// Three facts, each checkable now that a pool is real, and each a
/// separate rejection:
///
/// 1. `P` names a pool this image binds by `img.dma_pool` — not an
///    unbound name, and not an `img.pool`, whose memory no device can
///    reach (03 §3: "All memory a device can reach originates from its
///    bound pools").
/// 2. That pool is reachable from **this** binding's own device. A
///    `DmaPool[P, N]` is authority over device-reachable memory, and §1's
///    "the device is named once, at the image binding" is only a single
///    source of truth if the pool and the driver agree about which device
///    that is.
/// 3. `N` covers the backing the pool actually reserves — §1 calls it "a
///    **bounded** device-reachable pool", and a bound smaller than the
///    memory it admits authority over is a false one.
///
/// `N` is checked only when it is an integer literal, which is the only
/// spelling the language has today (`256.KiB` in §1's example is a stdlib
/// method call this compiler's prelude does not ship). Anything else fails
/// closed by name rather than being waved through — an unchecked bound is
/// exactly the thing this rule exists to prevent.
///
/// Like `DeviceCap`'s, this is the *typed* half: no runtime value is
/// produced, because `layout::build_boot_init_calls` never calls a
/// driver's `init` at all yet, and it fails closed by name on a pool
/// handle argument rather than passing one (M6-D's floor, unchanged).
fn check_dma_pool_mint(
    decl_ref: &ImageDeclRef,
    struct_name: &str,
    param_name: &str,
    param_ty: &Type,
    args: &[DeclArg],
    backings: &BTreeMap<String, PoolBacking>,
) -> Result<(), SemaError> {
    let rendered = types::render_type(param_ty);
    let targs = match param_ty {
        Type::Named(_, targs) => targs.as_slice(),
        _ => &[][..],
    };
    let Some(crate::sema::types::TypeArg::Pool(pool)) = targs.first() else {
        // Unrepresentable from source: `sema::types::resolve_named`
        // resolves `DmaPool`'s argument 0 against the declared pool names
        // and rejects anything else before this pass ever runs.
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init`'s own \
             `{param_name}: {rendered}` does not name a pool in its first argument \
             (03-hardware.md §1)",
            decl_ref.render()
        )));
    };
    let Some(b) = backings.get(pool) else {
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init` takes `{param_name}: \
             {rendered}` and this image never binds pool `{pool}` with `img.dma_pool` — \
             03-hardware.md §3: all memory a device can reach originates from its bound pools",
            decl_ref.render()
        )));
    };
    let Some(pool_device) = b.device else {
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init` takes `{param_name}: \
             {rendered}` and pool `{pool}` is bound by `img.pool`, which declares no device — a \
             `DmaPool[P, N]` is authority over *device-reachable* memory (03-hardware.md §1/§3). \
             Bind it with `img.dma_pool(name={pool}, device=..., count=...)` instead",
            decl_ref.render()
        )));
    };
    // 03 §1: "The device itself is named once, at the image binding ...
    // the single source of truth" — which is only true if the pool and
    // the driver name the same one.
    let bound_device = match args.iter().find(|a| a.label == "device").map(|a| &a.value) {
        Some(Value::ImageDecl(ImageDeclRef::Device(i))) => Some(*i),
        _ => None,
    };
    match bound_device {
        None => {
            return Err(build_error(format!(
                "`{}` declares no `device=`, but `{struct_name}.init` takes `{param_name}: \
                 {rendered}` — a capability is minted while the image binds a declared device to \
                 a `@driver`, and pool `{pool}` is reachable from device#{pool_device} \
                 (03-hardware.md §1)",
                decl_ref.render()
            )));
        }
        Some(i) if i != pool_device => {
            return Err(build_error(format!(
                "`{}` binds device#{i} to `{struct_name}`, but `{struct_name}.init` takes \
                 `{param_name}: {rendered}` and pool `{pool}` is declared reachable from \
                 device#{pool_device} — a `DmaPool[P, N]` is authority over memory *this* \
                 device can reach (03-hardware.md §1/§3)",
                decl_ref.render()
            )));
        }
        Some(_) => {}
    }
    // "a **bounded** device-reachable pool" (03 §1).
    let Some(crate::sema::types::TypeArg::Const(bound_expr)) = targs.get(1) else {
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init`'s own \
             `{param_name}: {rendered}` declares no capacity bound `N` (03-hardware.md §1: a \
             `DmaPool[P, N]` is a *bounded* device-reachable pool)",
            decl_ref.render()
        )));
    };
    let crate::syntax::ast::Expr::Int(_, digits) = bound_expr else {
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, and `{struct_name}.init` takes `{param_name}: \
             {rendered}` — but this compiler can only check a capacity bound written as an \
             integer literal, and `{}` is not one. Failing closed rather than admitting an \
             unchecked bound",
            decl_ref.render(),
            crate::syntax::printer::print_expr_bare(bound_expr)
        )));
    };
    let Ok(bound) = digits.replace('_', "").parse::<u64>() else {
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, and `{struct_name}.init`'s own \
             `{param_name}: {rendered}` declares a capacity bound this compiler cannot read as a \
             `u64`",
            decl_ref.render()
        )));
    };
    if b.bytes > bound {
        return Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init` takes `{param_name}: \
             {rendered}` while pool `{pool}` reserves {} bytes ({} slot(s) of {} bytes) — \
             03-hardware.md §1 calls this a *bounded* device-reachable pool, and a bound smaller \
             than the memory it admits authority over is not one",
            decl_ref.render(),
            b.bytes,
            b.slots,
            b.slot_bytes
        )));
    }
    Ok(())
}

fn check_one_decl(
    decl_ref: &ImageDeclRef,
    actor_type: &Type,
    args: &[DeclArg],
    kind: DeclKind,
    programs: &BTreeMap<String, TypedProgram>,
    graph: &ImageGraph,
    backings: &BTreeMap<String, PoolBacking>,
) -> Result<(), SemaError> {
    let Type::Named(struct_name, _) = actor_type else {
        return Ok(()); // defensive: only ever a bare struct name reaches here
    };
    let ctor = find_constructor(programs, actor_type);
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
    let mut device_caps_seen = 0usize;
    for (name, ty) in params {
        if satisfied.contains(name) {
            continue;
        }
        if let Type::Named(tn, _) = ty {
            // plans/M7.md item A: what was a bare name-recognition
            // "accepted, substituted by the declaration's own device
            // wiring" is now a real check against the wiring it names —
            // 03-hardware.md §1's mint, typed. `is_handle_type_name`
            // keeps its old behavior unchanged (plans/M4.md decision 7).
            if is_capability_type_name(tn) {
                check_capability_substitution(
                    decl_ref,
                    struct_name,
                    name,
                    ty,
                    tn,
                    kind,
                    args,
                    graph,
                    backings,
                    &mut device_caps_seen,
                )?;
                continue;
            }
            if is_handle_type_name(tn) {
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
    // plans/M7.md item D: `DmaPool[P, N]`'s mint reads the same resolved
    // pool table `check_pool_decls` (which ran first, in `check_sealed`'s
    // fixed order) already validated. Recomputed here rather than
    // threaded, so this fn keeps the signature every caller and every unit
    // test already uses; `pool_backings` is a pure function of the graph
    // and the closure's layouts, so the two calls cannot disagree.
    let backings = pool_backings(graph, &closure_layouts(programs)?)?;
    for (i, d) in graph.drivers.iter().enumerate() {
        check_one_decl(
            &ImageDeclRef::Driver(i),
            &d.actor_type,
            &d.args,
            DeclKind::Driver,
            programs,
            graph,
            &backings,
        )?;
    }
    for (i, d) in graph.actors.iter().enumerate() {
        check_one_decl(
            &ImageDeclRef::Actor(i),
            &d.actor_type,
            &d.args,
            DeclKind::Actor,
            programs,
            graph,
            &backings,
        )?;
    }
    Ok(())
}

// --- plans/M7.md item G: vector binding (03-hardware.md §6) ----------------
//
// "The ownership unit is a **vector**: exactly one handler per vector,
// possibly several vectors per driver ... The vector table is generated
// from the image graph; source cannot bind an unowned vector."
//
// Decision 12: an `IrqCap[V]`'s runtime word is the vector bit index
// (1..=63; bit 0 is M6's deadline/cancel vector). A device declares its
// vector with optional `vector=N` on `img.device`; `take_irq` /
// `IrqCap.bind` are rejected when that declaration is absent (unowned).
//
// Three named rejections, each a golden:
//   - a vector bound twice (`golden/err-irq-vector-bound-twice`)
//   - two vectors bound to one handler (`golden/err-irq-two-vectors-one-handler`)
//   - binding / taking an unowned vector (`golden/err-irq-unowned-vector`)

/// Bit 0 is reserved for M6's deadline/cancel vector
/// (`__wrela_vector0_service`). A device-owned vector is any other bit.
pub const DEADLINE_VECTOR: u64 = 0;

/// The `vector=N` an `img.device[...](..., vector=N)` declaration names,
/// if any. `None` is 03 §7's poll build: no vector exists for this device.
pub(crate) fn device_vector(args: &[DeclArg]) -> Option<u64> {
    let a = args.iter().find(|a| a.label == "vector")?;
    match &a.value {
        Value::U8(n) => Some(*n as u64),
        Value::U16(n) => Some(*n as u64),
        Value::U32(n) => Some(*n as u64),
        Value::U64(n) => Some(*n),
        Value::Usize(n) => Some(*n),
        Value::I8(n) if *n >= 0 => Some(*n as u64),
        Value::I16(n) if *n >= 0 => Some(*n as u64),
        Value::I32(n) if *n >= 0 => Some(*n as u64),
        Value::I64(n) if *n >= 0 => Some(*n as u64),
        Value::Isize(n) if *n >= 0 => Some(*n as u64),
        _ => None,
    }
}

/// An `IrqCap[V]` `init` parameter: minted from the bound device's
/// `vector=` (decision 12). Prefer `claimed.take_irq()` inside `init`.
fn check_irq_cap_mint(
    decl_ref: &ImageDeclRef,
    struct_name: &str,
    param_name: &str,
    param_ty: &Type,
    args: &[DeclArg],
    graph: &ImageGraph,
) -> Result<(), SemaError> {
    let rendered = types::render_type(param_ty);
    let Some(device_arg) = args.iter().find(|a| a.label == "device") else {
        return Err(build_error(format!(
            "`{}` declares no `device=`, but `{struct_name}.init` takes `{param_name}: {rendered}` \
             — an `IrqCap[V]` is minted from a device's declared `vector=` (03-hardware.md §6)",
            decl_ref.render()
        )));
    };
    let Value::ImageDecl(ImageDeclRef::Device(idx)) = &device_arg.value else {
        return Err(build_error(format!(
            "`{}` passes a `device=` that is not a declared device, so `{struct_name}.init`'s own \
             `{param_name}: {rendered}` has no vector to mint from",
            decl_ref.render()
        )));
    };
    let Some(dev) = graph.devices.get(*idx) else {
        return Err(build_error(format!(
            "`{}` binds device#{idx}, which this image does not declare",
            decl_ref.render()
        )));
    };
    match device_vector(&dev.args) {
        Some(v) if v != DEADLINE_VECTOR && v <= 63 => Ok(()),
        Some(DEADLINE_VECTOR) => Err(build_error(format!(
            "`{}` binds device#{idx} with `vector=0`, but bit 0 is reserved for the deadline/\
             cancel vector (06-machine.md §4 / plans/M6.md item E); a device-owned vector is \
             `vector=1..=63`",
            decl_ref.render()
        ))),
        Some(v) => Err(build_error(format!(
            "`{}` binds device#{idx} with `vector={v}`, which does not fit the per-core pending \
             word (bits 0..=63; bit 0 is the deadline vector)",
            decl_ref.render()
        ))),
        None => Err(build_error(format!(
            "`{}` binds a device to `{struct_name}`, but `{struct_name}.init` takes `{param_name}: \
             {rendered}` and the device declared no `vector=` — 03-hardware.md §6: source cannot \
             bind an unowned vector. Add `vector=N` (1..=63) to the `img.device` call, or drop \
             the `IrqCap` parameter and use a poll build (03-hardware.md §7)",
            decl_ref.render()
        ))),
    }
}

/// One static `IrqCap.bind(handler)` site found in a `@driver`'s body.
struct IrqBindSite {
    driver: String,
    vector: u64,
    handler: String,
    site: String,
}

/// 03-hardware.md §6's vector-table rule, checked over the sealed graph.
pub fn check_vector_bindings(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    // First: every device's `vector=` is well-formed and unique across
    // the image (two devices may not share a pending-word bit).
    let mut claimed: BTreeMap<u64, usize> = BTreeMap::new();
    for (i, d) in graph.devices.iter().enumerate() {
        let Some(v) = device_vector(&d.args) else {
            continue;
        };
        if v == DEADLINE_VECTOR {
            return Err(build_error(format!(
                "`device#{i}` declares `vector=0`, but bit 0 is reserved for the deadline/cancel \
                 vector (06-machine.md §4); a device-owned vector is `vector=1..=63`"
            )));
        }
        if v > 63 {
            return Err(build_error(format!(
                "`device#{i}` declares `vector={v}`, which does not fit the per-core pending word \
                 (bits 0..=63)"
            )));
        }
        if let Some(prev) = claimed.insert(v, i) {
            return Err(build_error(format!(
                "`device#{i}` and `device#{prev}` both declare `vector={v}` — 03-hardware.md §6: \
                 exactly one handler per vector, and a vector is owned by exactly one device"
            )));
        }
    }

    // Collect every bind / take_irq site from every declared driver's
    // typed bodies. A free fn cannot hold an `IrqCap` (containment), so
    // only driver members are walked.
    let mut binds: Vec<IrqBindSite> = Vec::new();
    let mut take_irq_sites: Vec<(String, Option<u64>, String)> = Vec::new();
    for (di, decl) in graph.drivers.iter().enumerate() {
        let Type::Named(driver, targs) = &decl.actor_type else {
            continue;
        };
        let vector = device_index_of_driver(decl)
            .and_then(|i| graph.devices.get(i).and_then(|d| device_vector(&d.args)));
        let Some(program) = programs.values().find(|p| {
            p.structs.contains_key(driver)
                || p.instantiations.keys().any(|k| {
                    k.strip_prefix("struct:").unwrap_or(k).split('[').next()
                        == Some(driver.as_str())
                })
        }) else {
            continue;
        };
        let mut walk = |site_key: String, f: &crate::sema::typed::TypedFn| {
            collect_irq_ops(
                &f.body,
                driver,
                vector,
                &site_key,
                &mut binds,
                &mut take_irq_sites,
            );
        };
        // Plain (non-generic) driver: bodies live on `program.structs`.
        if targs.is_empty() {
            if let Some(s) = program.structs.get(driver) {
                if let Some(f) = &s.init {
                    walk(format!("{driver}.init"), f);
                }
                for (m, f) in &s.methods {
                    walk(format!("{driver}.{m}"), f);
                }
                for (m, f) in &s.assoc_fns {
                    walk(format!("{driver}.{m}"), f);
                }
            }
        } else {
            // plans/M7.md item G, decision 18: mode-generic driver bodies
            // live on the instantiation (`struct:BlkDriver[DriverMode.Irq]`).
            let key = format!(
                "struct:{}",
                crate::sema::types::render_type(&decl.actor_type)
            );
            if let Some(crate::sema::typed::TypedInstantiation::Struct(s)) =
                program.instantiations.get(&key)
            {
                if let Some(f) = &s.init {
                    walk(format!("{key}.init"), f);
                }
                for (m, f) in &s.methods {
                    walk(format!("{key}.{m}"), f);
                }
                for (m, f) in &s.assoc_fns {
                    walk(format!("{key}.{m}"), f);
                }
            }
        }
        let _ = di;
    }

    // Unowned: take_irq or bind against a device with no vector=.
    for (driver, vector, site) in &take_irq_sites {
        if vector.is_none() {
            return Err(build_error(format!(
                "`{site}` calls `take_irq()`, but `@driver` `{driver}`'s device declared no \
                 `vector=` — 03-hardware.md §6: source cannot bind an unowned vector. Add \
                 `vector=N` (1..=63) to the `img.device` call, or drop the call for a poll build \
                 (03-hardware.md §7)"
            )));
        }
    }
    for b in &binds {
        if device_vector_for_driver(graph, &b.driver).is_none() {
            return Err(build_error(format!(
                "`{}` binds handler `{}`, but `@driver` `{}`'s device declared no `vector=` — \
                 03-hardware.md §6: source cannot bind an unowned vector",
                b.site, b.handler, b.driver
            )));
        }
    }

    // A vector bound twice: two bind sites naming the same vector bit.
    let mut by_vector: BTreeMap<u64, &IrqBindSite> = BTreeMap::new();
    for b in &binds {
        if let Some(prev) = by_vector.insert(b.vector, b) {
            return Err(build_error(format!(
                "vector {} is bound twice — first in `{}` to `{}`, then in `{}` to `{}` \
                 (03-hardware.md §6: exactly one handler per vector)",
                b.vector, prev.site, prev.handler, b.site, b.handler
            )));
        }
    }

    // Two vectors bound to one handler.
    let mut by_handler: BTreeMap<&str, &IrqBindSite> = BTreeMap::new();
    for b in &binds {
        if let Some(prev) = by_handler.insert(b.handler.as_str(), b) {
            if prev.vector != b.vector {
                return Err(build_error(format!(
                    "handler `{}` is bound to two vectors ({} via `{}`, {} via `{}`) — \
                     03-hardware.md §6: exactly one handler per vector, and this compiler \
                     rejects one handler serving two",
                    b.handler, prev.vector, prev.site, b.vector, b.site
                )));
            }
        }
    }
    Ok(())
}

fn device_index_of_driver(decl: &crate::eval::image::DriverDecl) -> Option<usize> {
    decl.args
        .iter()
        .find(|a| a.label == "device")
        .and_then(|a| match &a.value {
            Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
            _ => None,
        })
}

fn device_vector_for_driver(graph: &ImageGraph, driver: &str) -> Option<u64> {
    for decl in &graph.drivers {
        let Type::Named(name, _) = &decl.actor_type else {
            continue;
        };
        if name != driver {
            continue;
        }
        return device_index_of_driver(decl)
            .and_then(|i| graph.devices.get(i))
            .and_then(|d| device_vector(&d.args));
    }
    None
}

fn collect_irq_ops(
    stmts: &[crate::sema::typed::TypedStmt],
    driver: &str,
    vector: Option<u64>,
    site: &str,
    binds: &mut Vec<IrqBindSite>,
    take_irq: &mut Vec<(String, Option<u64>, String)>,
) {
    for s in stmts {
        collect_irq_ops_stmt(s, driver, vector, site, binds, take_irq);
    }
}

fn collect_irq_ops_stmt(
    stmt: &crate::sema::typed::TypedStmt,
    driver: &str,
    vector: Option<u64>,
    site: &str,
    binds: &mut Vec<IrqBindSite>,
    take_irq: &mut Vec<(String, Option<u64>, String)>,
) {
    use crate::sema::typed::{TypedDeferBody, TypedForIter, TypedStmtKind};
    match &stmt.kind {
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::Let { value: e, .. } => {
            collect_irq_ops_expr(e, driver, vector, site, binds, take_irq);
        }
        TypedStmtKind::Assign { target, value } => {
            collect_irq_ops_expr(target, driver, vector, site, binds, take_irq);
            collect_irq_ops_expr(value, driver, vector, site, binds, take_irq);
        }
        TypedStmtKind::Return(Some(e)) => {
            collect_irq_ops_expr(e, driver, vector, site, binds, take_irq);
        }
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            collect_irq_ops_expr(cond, driver, vector, site, binds, take_irq);
            if let Some(m) = message {
                collect_irq_ops_expr(m, driver, vector, site, binds, take_irq);
            }
        }
        TypedStmtKind::Return(None)
        | TypedStmtKind::Break
        | TypedStmtKind::Continue
        | TypedStmtKind::Pass => {}
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            collect_irq_ops_expr(cond, driver, vector, site, binds, take_irq);
            collect_irq_ops(then_branch, driver, vector, site, binds, take_irq);
            for elif in elifs {
                collect_irq_ops_expr(&elif.cond, driver, vector, site, binds, take_irq);
                collect_irq_ops(&elif.body, driver, vector, site, binds, take_irq);
            }
            if let Some(b) = else_branch {
                collect_irq_ops(b, driver, vector, site, binds, take_irq);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            collect_irq_ops_expr(scrutinee, driver, vector, site, binds, take_irq);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_irq_ops_expr(g, driver, vector, site, binds, take_irq);
                }
                collect_irq_ops(&arm.body, driver, vector, site, binds, take_irq);
            }
        }
        TypedStmtKind::While { cond, body } => {
            collect_irq_ops_expr(cond, driver, vector, site, binds, take_irq);
            collect_irq_ops(body, driver, vector, site, binds, take_irq);
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(start, end, _) => {
                    collect_irq_ops_expr(start, driver, vector, site, binds, take_irq);
                    collect_irq_ops_expr(end, driver, vector, site, binds, take_irq);
                }
                TypedForIter::Expr(e) => {
                    collect_irq_ops_expr(e, driver, vector, site, binds, take_irq);
                }
            }
            collect_irq_ops(body, driver, vector, site, binds, take_irq);
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => {
                collect_irq_ops_expr(e, driver, vector, site, binds, take_irq);
            }
            TypedDeferBody::Suite(stmts) => {
                collect_irq_ops(stmts, driver, vector, site, binds, take_irq);
            }
        },
        TypedStmtKind::BareSend { expr, .. } => {
            collect_irq_ops_expr(expr, driver, vector, site, binds, take_irq);
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                collect_irq_ops_expr(c, driver, vector, site, binds, take_irq);
            }
            if let Some(d) = deadline {
                collect_irq_ops_expr(d, driver, vector, site, binds, take_irq);
            }
            collect_irq_ops(body, driver, vector, site, binds, take_irq);
        }
    }
}

fn collect_irq_ops_expr(
    e: &crate::sema::typed::TypedExpr,
    driver: &str,
    vector: Option<u64>,
    site: &str,
    binds: &mut Vec<IrqBindSite>,
    take_irq: &mut Vec<(String, Option<u64>, String)>,
) {
    use crate::sema::typed::{TypedClosureBody, TypedExprKind};
    match &e.kind {
        TypedExprKind::Intrinsic {
            key,
            args,
            receiver,
            ..
        } if key == "Device.take_irq" => {
            take_irq.push((driver.to_string(), vector, site.to_string()));
            if let Some(r) = receiver {
                collect_irq_ops_expr(r, driver, vector, site, binds, take_irq);
            }
            for (_, a) in args {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Intrinsic {
            key,
            args,
            receiver,
            ..
        } if key == "IrqCap.bind" => {
            let handler = args
                .iter()
                .find(|(l, _)| l == "handler")
                .and_then(|(_, h)| match &h.kind {
                    TypedExprKind::FnRef(k) => Some(k.spelling()),
                    _ => None,
                })
                .unwrap_or_else(|| "<unknown>".to_string());
            binds.push(IrqBindSite {
                driver: driver.to_string(),
                vector: vector.unwrap_or(0),
                handler,
                site: site.to_string(),
            });
            if let Some(r) = receiver {
                collect_irq_ops_expr(r, driver, vector, site, binds, take_irq);
            }
            for (_, a) in args {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                collect_irq_ops_expr(r, driver, vector, site, binds, take_irq);
            }
            for (_, a) in args {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Field(b, _)
        | TypedExprKind::Take(b)
        | TypedExprKind::Neg(b)
        | TypedExprKind::BitNot(b)
        | TypedExprKind::Not(b)
        | TypedExprKind::ToScalar(b)
        | TypedExprKind::Await(b)
        | TypedExprKind::Send(b)
        | TypedExprKind::Panic(b) => {
            collect_irq_ops_expr(b, driver, vector, site, binds, take_irq);
        }
        TypedExprKind::Index(a, b)
        | TypedExprKind::Binary(_, a, b)
        | TypedExprKind::OpCall(_, a, b)
        | TypedExprKind::And(a, b)
        | TypedExprKind::Or(a, b) => {
            collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            collect_irq_ops_expr(b, driver, vector, site, binds, take_irq);
        }
        TypedExprKind::Is(a, _) => {
            collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                collect_irq_ops_expr(r, driver, vector, site, binds, take_irq);
            }
            for a in args.iter().flatten() {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::CallValue(f, args) => {
            collect_irq_ops_expr(f, driver, vector, site, binds, take_irq);
            for a in args {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Try(inner, _) => {
            collect_irq_ops_expr(inner, driver, vector, site, binds, take_irq);
        }
        TypedExprKind::EnumConstruct { args, .. }
        | TypedExprKind::Tuple(args)
        | TypedExprKind::List(args) => {
            for a in args {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, a) in fields {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => {
                collect_irq_ops_expr(e, driver, vector, site, binds, take_irq);
            }
            TypedClosureBody::Suite(stmts) => {
                collect_irq_ops(stmts, driver, vector, site, binds, take_irq);
            }
        },
        _ => {}
    }
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

    // --- pool declarations (plans/M7.md item D) ---------------------------
    //
    // Goldens cover the shapes an `@image` fn can spell:
    // `err-dma-pool-payload-not-layout`, `err-dma-pool-payload-wrong-kind`,
    // `err-dma-pool-no-device`, `err-dma-pool-device-not-a-device`,
    // `err-pool-bound-twice-forms`, `err-dma-pool-own-mismatch`,
    // `err-pool-backing-too-large`, and `check-dma-pool` for the accept.
    // These unit tests cover the arms *no* source can currently reach —
    // the same discipline the construction-DAG cycle and the second-
    // `DeviceCap` arms above already follow — plus the two derivations
    // (`layout_alignment`, the exact size) that every later consumer rests
    // on.

    fn dma_layout(name: &str, fields: &[(&str, u64)]) -> types::LayoutType {
        let mut at = 0u64;
        let entries = fields
            .iter()
            .map(|(n, size)| {
                let f = types::LayoutEntry::Field(types::LayoutField {
                    name: n.to_string(),
                    ty: format!("u{}", size * 8),
                    offset: at,
                    size: *size,
                });
                at += size;
                f
            })
            .collect();
        types::LayoutType {
            name: name.to_string(),
            kind: types::LayoutKind::Dma,
            endian: types::LayoutEndian::Little,
            size: Some(at),
            padding: 0,
            entries,
        }
    }

    fn layouts_of(ls: Vec<types::LayoutType>) -> BTreeMap<String, types::LayoutType> {
        ls.into_iter().map(|l| (l.name.clone(), l)).collect()
    }

    /// A graph with one device, one driver bound to it (unless
    /// `driver_bound` is false), and one DMA pool with `args`.
    fn dma_pool_graph(driver_bound: bool, payload: &str, args: Vec<DeclArg>) -> ImageGraph {
        let mut g = ImageGraph::default();
        g.devices.push(crate::eval::image::DeviceDecl {
            device_type: Type::Named("BlockHw".to_string(), vec![]),
            args: vec![],
        });
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: if driver_bound {
                vec![decl_arg(
                    "device",
                    Type::Named("ImageDecl".to_string(), vec![]),
                    Value::ImageDecl(ImageDeclRef::Device(0)),
                )]
            } else {
                vec![]
            },
        });
        g.dma_pools.insert(
            "P".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::Named(payload.to_string(), vec![]),
                args,
            },
        );
        g
    }

    fn handle_arg(label: &str, r: ImageDeclRef) -> DeclArg {
        decl_arg(
            label,
            Type::Named("ImageDecl".to_string(), vec![]),
            Value::ImageDecl(r),
        )
    }

    #[test]
    fn layout_alignment_is_the_widest_scalar_field_clamped_to_a_machine_word() {
        assert_eq!(layout_alignment(&dma_layout("A", &[("a", 1)])), 1);
        assert_eq!(layout_alignment(&dma_layout("B", &[("a", 1), ("b", 2)])), 2);
        assert_eq!(layout_alignment(&dma_layout("C", &[("a", 4)])), 4);
        assert_eq!(layout_alignment(&dma_layout("D", &[("a", 4), ("b", 8)])), 8);
        // A `Bytes[N]` field is N bytes wide but needs no more than byte
        // alignment, so the clamp is what keeps this the machine's own
        // widest scalar alignment rather than an array's length.
        assert_eq!(layout_alignment(&dma_layout("E", &[("a", 512)])), 8);
    }

    #[test]
    fn a_dma_pool_reserves_exactly_count_times_the_payloads_own_bytes() {
        let g = dma_pool_graph(
            true,
            "Hdr",
            vec![
                handle_arg("device", ImageDeclRef::Device(0)),
                decl_arg("count", Type::I64, Value::I64(8)),
            ],
        );
        let layouts = layouts_of(vec![dma_layout(
            "Hdr",
            &[("kind", 4), ("reserved", 4), ("sector", 8)],
        )]);
        let b = pool_backings(&g, &layouts).expect("a well-formed DMA pool");
        let p = &b["P"];
        assert_eq!((p.slots, p.slot_bytes, p.bytes, p.align), (8, 16, 128, 8));
        assert!(p.is_dma);
        assert_eq!(p.device, Some(0));
    }

    #[test]
    fn a_dma_pool_may_name_its_device_through_the_driver_bound_to_it() {
        // 03-hardware.md §3's own worked example spells `device=disk`,
        // naming the *driver*; 05-library.md §9 spells `device=d`, naming
        // the device. Both resolve to the same declared device.
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        for r in [ImageDeclRef::Device(0), ImageDeclRef::Driver(0)] {
            let g = dma_pool_graph(
                true,
                "Hdr",
                vec![
                    handle_arg("device", r),
                    decl_arg("count", Type::I64, Value::I64(2)),
                ],
            );
            let b = pool_backings(&g, &layouts).expect("both spellings resolve");
            assert_eq!(b["P"].device, Some(0));
        }
    }

    #[test]
    fn a_dma_pool_named_through_a_driver_that_binds_no_device_is_rejected() {
        // Unrepresentable from source: `check_init_args` rejects a driver
        // whose `init` needs a `DeviceCap` with no `device=`, but a driver
        // with no capability parameter at all may legally bind no device,
        // and today nothing else stops it — so this is real code with a
        // real oracle and no golden, exactly like the construction-DAG
        // cycle case.
        let g = dma_pool_graph(
            false,
            "Hdr",
            vec![
                handle_arg("device", ImageDeclRef::Driver(0)),
                decl_arg("count", Type::I64, Value::I64(2)),
            ],
        );
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        let err = pool_backings(&g, &layouts).expect_err("that driver reaches no device");
        assert!(
            err.message.contains("binds no declared device"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_dma_pool_naming_an_undeclared_device_index_is_rejected() {
        let g = dma_pool_graph(
            true,
            "Hdr",
            vec![
                handle_arg("device", ImageDeclRef::Device(7)),
                decl_arg("count", Type::I64, Value::I64(2)),
            ],
        );
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        let err = pool_backings(&g, &layouts).expect_err("device#7 does not exist");
        assert!(err.message.contains("does not declare"), "{}", err.message);
    }

    /// plans/M8.md item P, decision 24: the inverse of the M7 post-closure
    /// guard this replaced. A pool bound to a driverless device, and a
    /// second pool bound to a *different*, driven one, both resolve — each
    /// carrying its own `device`, which is the fact the `BlkPool device=`
    /// mapping line and `GuestMem`'s per-device `window_offset` are built
    /// on. Neither shape is refused here any more; reachability is
    /// enforced where reaching happens.
    #[test]
    fn pools_on_two_devices_one_of_them_driverless_each_keep_their_own_device() {
        let mut g = ImageGraph::default();
        g.devices.push(crate::eval::image::DeviceDecl {
            device_type: Type::Named("BlockHw".to_string(), vec![]),
            args: vec![],
        });
        g.devices.push(crate::eval::image::DeviceDecl {
            device_type: Type::Named("OtherHw".to_string(), vec![]),
            args: vec![],
        });
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![handle_arg("device", ImageDeclRef::Device(0))],
        });
        for (name, dev) in [("Control", 0usize), ("Other", 1usize)] {
            g.dma_pools.insert(
                name.to_string(),
                crate::eval::image::PoolDecl {
                    payload_type: Type::Named("Hdr".to_string(), vec![]),
                    args: vec![
                        handle_arg("device", ImageDeclRef::Device(dev)),
                        decl_arg("count", Type::I64, Value::I64(2)),
                    ],
                },
            );
        }
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 8)])]);
        let out = pool_backings(&g, &layouts).expect("both pools resolve");
        assert_eq!(out["Control"].device, Some(0));
        assert_eq!(out["Other"].device, Some(1));
    }

    #[test]
    fn a_pool_argument_this_intrinsic_does_not_define_is_rejected() {
        let g = dma_pool_graph(
            true,
            "Hdr",
            vec![
                handle_arg("device", ImageDeclRef::Device(0)),
                decl_arg("count", Type::I64, Value::I64(2)),
                decl_arg("slots", Type::I64, Value::I64(2)),
            ],
        );
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        let err = pool_backings(&g, &layouts).expect_err("`slots=` is the other form's argument");
        assert!(
            err.message.contains("has no `slots=` argument"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_pool_capacity_must_be_a_positive_integer() {
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        for (value, want) in [
            (Value::I64(0), "at least one slot"),
            (Value::I64(-1), "at least one slot"),
            (Value::Bool(true), "is not an integer"),
        ] {
            let g = dma_pool_graph(
                true,
                "Hdr",
                vec![
                    handle_arg("device", ImageDeclRef::Device(0)),
                    decl_arg("count", Type::I64, value),
                ],
            );
            let err = pool_backings(&g, &layouts).expect_err("a pool reserves exact backing");
            assert!(err.message.contains(want), "{}", err.message);
        }
    }

    #[test]
    fn a_missing_capacity_argument_is_never_guessed() {
        let mut g = ImageGraph::default();
        g.pools.insert(
            "Buffers".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::U32,
                args: vec![decl_arg("max_payload", Type::I64, Value::I64(64))],
            },
        );
        let err =
            pool_backings(&g, &BTreeMap::new()).expect_err("`slots=` has no default and no guess");
        assert!(
            err.message.contains("declares no `slots=`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn the_whole_images_pool_backing_is_capped() {
        let mut g = ImageGraph::default();
        // Two pools, each individually placeable, together past the
        // ceiling: the check is on the total, because the total is what
        // gets emitted as image bytes.
        for name in ["A", "B"] {
            g.pools.insert(
                name.to_string(),
                crate::eval::image::PoolDecl {
                    payload_type: Type::U32,
                    args: vec![
                        decl_arg("slots", Type::I64, Value::I64(1024)),
                        decl_arg("max_payload", Type::I64, Value::I64(12 * 1024)),
                    ],
                },
            );
        }
        let err = pool_backings(&g, &BTreeMap::new()).expect_err("past MAX_POOL_BYTES");
        assert!(err.message.contains("ceiling"), "{}", err.message);
    }

    /// plans/M7.md item D self-audit: every arm below is reachable from
    /// real source (each was written as a `.wr` file and run through the
    /// real compiler during the audit) but has no golden of its own,
    /// because each is a one-line variant of a case that does. Kept as
    /// executable oracles rather than as a comment, which is the whole
    /// point of the audit.
    #[test]
    fn the_reachable_pool_arms_no_golden_spells() {
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);

        // `device=1` — an ordinary value, not a declaration reference.
        let g = dma_pool_graph(
            true,
            "Hdr",
            vec![
                decl_arg("device", Type::I64, Value::I64(1)),
                decl_arg("count", Type::I64, Value::I64(2)),
            ],
        );
        let err = pool_backings(&g, &layouts).expect_err("1 is not a declared device");
        assert!(
            err.message.contains("must name a declared device"),
            "{}",
            err.message
        );

        // `img.pool` with an argument belonging to the DMA form.
        let mut g2 = ImageGraph::default();
        g2.pools.insert(
            "B".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::U32,
                args: vec![
                    decl_arg("slots", Type::I64, Value::I64(2)),
                    decl_arg("max_payload", Type::I64, Value::I64(8)),
                    decl_arg("count", Type::I64, Value::I64(2)),
                ],
            },
        );
        let err = pool_backings(&g2, &BTreeMap::new()).expect_err("`count=` is the DMA form's");
        assert!(
            err.message.contains("has no `count=` argument"),
            "{}",
            err.message
        );

        // Backing whose product does not fit a `u64` at all — caught
        // before the ceiling check, which is a comparison and would
        // otherwise be handed a wrapped number.
        let mut g3 = ImageGraph::default();
        g3.pools.insert(
            "B".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::U32,
                args: vec![
                    decl_arg("slots", Type::I64, Value::I64(1 << 62)),
                    decl_arg("max_payload", Type::I64, Value::I64(1 << 62)),
                ],
            },
        );
        let err = pool_backings(&g3, &BTreeMap::new()).expect_err("past a u64");
        assert!(err.message.contains("more than a `u64`"), "{}", err.message);

        let mut g4 = dma_pool_graph(
            true,
            "Hdr",
            vec![
                handle_arg("device", ImageDeclRef::Device(0)),
                decl_arg("count", Type::I64, Value::I64(1 << 62)),
            ],
        );
        let wide = layouts_of(vec![types::LayoutType {
            name: "Hdr".to_string(),
            kind: types::LayoutKind::Dma,
            endian: types::LayoutEndian::Little,
            size: Some(1 << 62),
            padding: 0,
            entries: vec![],
        }]);
        g4.dma_pools.get_mut("P").expect("P is bound").payload_type =
            Type::Named("Hdr".to_string(), vec![]);
        let err = pool_backings(&g4, &wide).expect_err("past a u64");
        assert!(err.message.contains("more than a `u64`"), "{}", err.message);
    }

    /// The two `DmaPool[P, N]` bound-argument arms a source can spell but
    /// no golden pins (audited the same way): a bound that is a *type*
    /// rather than a const, and one this compiler cannot read as a `u64`.
    #[test]
    fn a_dma_pool_bound_must_be_a_readable_const() {
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        let g = dma_pool_graph(
            true,
            "Hdr",
            vec![
                handle_arg("device", ImageDeclRef::Device(0)),
                decl_arg("count", Type::I64, Value::I64(2)),
            ],
        );
        let backings = pool_backings(&g, &layouts).expect("well-formed");
        let span = crate::syntax::ast::Span { line: 1, col: 1 };

        let ty = |second: crate::sema::types::TypeArg| {
            Type::Named(
                "DmaPool".to_string(),
                vec![crate::sema::types::TypeArg::Pool("P".to_string()), second],
            )
        };
        let err = check_dma_pool_mint(
            &ImageDeclRef::Driver(0),
            "Blk",
            "control",
            &ty(crate::sema::types::TypeArg::Type(Type::U32)),
            &g.drivers[0].args,
            &backings,
        )
        .expect_err("a type is not a capacity bound");
        assert!(
            err.message.contains("declares no capacity bound `N`"),
            "{}",
            err.message
        );

        let err = check_dma_pool_mint(
            &ImageDeclRef::Driver(0),
            "Blk",
            "control",
            &ty(crate::sema::types::TypeArg::Const(
                crate::syntax::ast::Expr::Int(span, "9".repeat(30)),
            )),
            &g.drivers[0].args,
            &backings,
        )
        .expect_err("does not fit a u64");
        assert!(
            err.message.contains("cannot read as a `u64`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn own_handles_are_found_at_every_nesting_a_signature_can_reach() {
        // The agreement check is only as good as this walk, and a
        // signature can bury a handle arbitrarily deep
        // (`Result[Option[own[P] T], Wrapper[own[Q] U]]` is ordinary
        // wrela).
        let deep = Type::Result(
            Box::new(Type::Option(Box::new(Type::Own(
                "P".to_string(),
                Box::new(Type::U32),
            )))),
            Box::new(Type::Named(
                "Wrapper".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Own(
                    "Q".to_string(),
                    Box::new(Type::U8),
                ))],
            )),
        );
        let mut found = Vec::new();
        own_handles_in_type(&deep, &mut found);
        let names: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert!(names.contains(&"P"), "{names:?}");
        assert!(names.contains(&"Q"), "{names:?}");
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
                    is_task: false,
                    is_layout_assert: false,
                    is_pub: false,
                }),
                is_actor: true,
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

    // --- the mint (plans/M7.md item A, 03-hardware.md §1) ----------------
    //
    // What was, before item A, a bare name-recognition — "a parameter
    // whose declared type is named `DeviceCap`/`DmaPool`/`Mmio`/`IrqCap`
    // is satisfied by the declaration's own device wiring" (plans/M4.md
    // decision 7), accepted with *no argument and no wiring at all* — is
    // now a real check against the wiring it names. The old test asserted
    // exactly that acceptance and is replaced, not deleted, by the table
    // below: same shape, real rule.
    //
    // These stay unit tests rather than becoming goldens for the arms a
    // golden cannot reach: `check_init_args` runs on a *sealed* graph, and
    // the driver-shaped rejections that can be spelled in source are
    // goldens (`golden/err-cap-mint-no-device`,
    // `golden/err-cap-mint-device-mismatch`, `golden/err-cap-mint-unminted`),
    // while the second-`DeviceCap` and non-device-`device=` arms need a
    // graph shape no `@image` fn can currently produce.

    fn cap_param(name: &str, cap: &str, arg: &str) -> TypedParam {
        TypedParam {
            mode: AccessMode::Read,
            name: name.to_string(),
            ty: Type::Named(
                cap.to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Named(
                    arg.to_string(),
                    vec![],
                ))],
            ),
            default: None,
        }
    }

    /// One driver graph: a device of type `device_type` (if any), and a
    /// driver wired with `device=` (if `wired`) plus `params` on its init.
    fn driver_graph(device_type: Option<&str>, wired: bool, params: Vec<TypedParam>) -> ImageGraph {
        let mut g = ImageGraph::default();
        if let Some(d) = device_type {
            g.devices.push(crate::eval::image::DeviceDecl {
                device_type: Type::Named(d.to_string(), vec![]),
                args: vec![],
            });
        }
        let _ = &params;
        let args = if wired {
            vec![decl_arg(
                "device",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Device(0)),
            )]
        } else {
            vec![]
        };
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args,
        });
        g
    }

    #[test]
    fn a_device_cap_is_minted_by_the_binding_that_names_its_device() {
        let programs = programs_map(program_with_init(
            "Blk",
            vec![cap_param("cap", "DeviceCap", "BlockHw")],
        ));
        let g = driver_graph(Some("BlockHw"), true, vec![]);
        assert!(
            check_init_args(&g, &programs).is_ok(),
            "a `DeviceCap[BlockHw]` bound to an `img.device[BlockHw]` is 03 §1's own mint"
        );
    }

    #[test]
    fn a_device_cap_with_no_device_binding_has_nothing_to_be_minted_from() {
        let programs = programs_map(program_with_init(
            "Blk",
            vec![cap_param("cap", "DeviceCap", "BlockHw")],
        ));
        let g = driver_graph(Some("BlockHw"), false, vec![]);
        let err = check_init_args(&g, &programs).expect_err("no `device=` means no mint");
        assert!(
            err.message.contains("declares no `device=`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_device_cap_must_name_the_bound_device_type() {
        let programs = programs_map(program_with_init(
            "Blk",
            vec![cap_param("cap", "DeviceCap", "NicHw")],
        ));
        let g = driver_graph(Some("BlockHw"), true, vec![]);
        let err = check_init_args(&g, &programs).expect_err("`D` must be the bound device's type");
        assert!(
            err.message.contains("`NicHw` is not `BlockHw`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn one_binding_mints_at_most_one_device_cap() {
        let programs = programs_map(program_with_init(
            "Blk",
            vec![
                cap_param("cap", "DeviceCap", "BlockHw"),
                cap_param("cap2", "DeviceCap", "BlockHw"),
            ],
        ));
        let g = driver_graph(Some("BlockHw"), true, vec![]);
        let err = check_init_args(&g, &programs).expect_err("one device, one authority");
        assert!(
            err.message.contains("more than one `DeviceCap` parameter"),
            "{}",
            err.message
        );
    }

    #[test]
    fn one_device_is_bound_by_at_most_one_driver() {
        // Post-closure M7 sweep port: the per-device half of §1's "named
        // once". Two drivers on device#0 used to place two DeviceRegs
        // windows at two bases.
        let mut g = ImageGraph::default();
        g.devices.push(crate::eval::image::DeviceDecl {
            device_type: Type::Named("BlockHw".to_string(), vec![]),
            args: vec![],
        });
        let wire = |ty: &str| crate::eval::image::DriverDecl {
            actor_type: Type::Named(ty.to_string(), vec![]),
            args: vec![decl_arg(
                "device",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Device(0)),
            )],
        };
        g.drivers.push(wire("A"));
        g.drivers.push(wire("B"));
        let err = check_device_bound_once(&g).expect_err("one device, one binding");
        assert!(
            err.message.contains("`driver#1` binds device#0")
                && err.message.contains("`driver#0` already binds"),
            "{}",
            err.message
        );
    }

    #[test]
    fn every_still_unminted_capability_names_the_item_that_mints_it() {
        // plans/M7.md item D minted `DmaPool[P, N]` and dropped it off
        // this list; `DmaShared[P, L]` joined it, with a different reason
        // (03-hardware.md §3's shared control memory is not minted at the
        // image binding at all — a queue configures it out of a pool).
        // `golden/err-cap-mint-unminted` and
        // `golden/err-dma-shared-unminted` are the source-shaped witnesses.
        //
        // plans/M7.md item H1 changed the `Mmio` arm's *kind*, which is
        // why it is checked against a mechanism rather than a plan item:
        // decision 10 found the old text ("that is item C") stale, because
        // an `Mmio[L]` is not waiting on an item at all — it is minted, by
        // the sealed transport's `map_partition`, and never at the image
        // binding. That rejection is permanent, so naming a future item in
        // it would be wrong forever rather than merely out of date.
        for (cap, owner) in [
            ("Mmio", "map_partition"),
            ("DmaShared", "VirtQueue.configure"),
        ] {
            let programs =
                programs_map(program_with_init("Blk", vec![cap_param("c", cap, "Thing")]));
            let g = driver_graph(Some("BlockHw"), true, vec![]);
            let err = check_init_args(&g, &programs)
                .expect_err("nothing mints an Mmio/DmaShared at the image binding");
            assert!(err.message.contains(owner), "{cap}: {}", err.message);
        }
        // IrqCap is item G: mintable from vector=, refused without one.
        let programs = programs_map(program_with_init(
            "Blk",
            vec![cap_param("c", "IrqCap", "Thing")],
        ));
        let g = driver_graph(Some("BlockHw"), true, vec![]);
        let err = check_init_args(&g, &programs).expect_err("IrqCap needs vector=");
        assert!(
            err.message.contains("vector=") || err.message.contains("unowned"),
            "IrqCap: {}",
            err.message
        );
        // plans/M7.md item G self-audit: an `IrqCap` init param with no
        // `device=` on the driver binding — source-unreachable for a
        // `@driver` that takes DeviceCap (sema demands the wiring), so
        // named here rather than as a golden.
        let g_no_dev = driver_graph(Some("BlockHw"), false, vec![]);
        let err =
            check_init_args(&g_no_dev, &programs).expect_err("IrqCap init param needs device=");
        assert!(
            err.message.contains("declares no `device=`"),
            "IrqCap no-device: {}",
            err.message
        );
    }

    /// The `DmaPool[P, N]` mint's own arms (plans/M7.md item D). The three
    /// a source can spell are goldens (`err-dma-pool-mint-bound`,
    /// `err-dma-pool-mint-not-dma`, `err-dma-pool-mint-wrong-device`);
    /// these are the two it cannot — a driver binding with no `device=`
    /// (sema rejects a `@driver` `init` reaching this shape earlier) and a
    /// bound spelled as something other than an integer literal, which the
    /// language has no way to write yet at all.
    #[test]
    fn a_dma_pool_mint_needs_a_device_binding_and_a_readable_bound() {
        let pool_param = |bound: crate::syntax::ast::Expr| TypedParam {
            mode: AccessMode::Take,
            name: "control".to_string(),
            ty: Type::Named(
                "DmaPool".to_string(),
                vec![
                    crate::sema::types::TypeArg::Pool("P".to_string()),
                    crate::sema::types::TypeArg::Const(bound),
                ],
            ),
            default: None,
        };
        let span = crate::syntax::ast::Span { line: 1, col: 1 };
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        let dma_args = vec![
            handle_arg("device", ImageDeclRef::Device(0)),
            decl_arg("count", Type::I64, Value::I64(2)),
        ];

        // No `device=` on the driver binding at all.
        let mut g = dma_pool_graph(false, "Hdr", dma_args.clone());
        g.drivers[0].args.clear();
        let backings = pool_backings(&g, &layouts).expect("the pool itself is well-formed");
        let programs = programs_map(program_with_init(
            "Blk",
            vec![pool_param(crate::syntax::ast::Expr::Int(
                span,
                "4096".to_string(),
            ))],
        ));
        let _ = &programs;
        let err = check_dma_pool_mint(
            &ImageDeclRef::Driver(0),
            "Blk",
            "control",
            &pool_param(crate::syntax::ast::Expr::Int(span, "4096".to_string())).ty,
            &g.drivers[0].args,
            &backings,
        )
        .expect_err("no device binding, nothing to mint from");
        assert!(
            err.message.contains("declares no `device=`"),
            "{}",
            err.message
        );

        // A bound this compiler cannot read as an integer literal.
        let g2 = dma_pool_graph(true, "Hdr", dma_args);
        let backings2 = pool_backings(&g2, &layouts).expect("well-formed");
        let err2 = check_dma_pool_mint(
            &ImageDeclRef::Driver(0),
            "Blk",
            "control",
            &pool_param(crate::syntax::ast::Expr::Name(span, "LIMIT".to_string())).ty,
            &g2.drivers[0].args,
            &backings2,
        )
        .expect_err("an unchecked bound is exactly what this rule prevents");
        assert!(err2.message.contains("integer literal"), "{}", err2.message);
    }

    #[test]
    fn a_device_argument_that_names_no_device_mints_nothing() {
        // `device=` wired to another *driver*'s handle rather than a
        // device. Unrepresentable from source today only because nothing
        // else is ever passed as `device=`, exactly like the
        // construction-DAG cycle case above — real code, real oracle, no
        // golden.
        let programs = programs_map(program_with_init(
            "Blk",
            vec![cap_param("cap", "DeviceCap", "BlockHw")],
        ));
        let mut g = ImageGraph::default();
        g.devices.push(crate::eval::image::DeviceDecl {
            device_type: Type::Named("BlockHw".to_string(), vec![]),
            args: vec![],
        });
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg(
                "device",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Driver(0)),
            )],
        });
        let err = check_init_args(&g, &programs).expect_err("a driver is not a device");
        assert!(
            err.message.contains("is not a declared device"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_device_argument_naming_an_undeclared_index_mints_nothing() {
        // The structural floor beneath the arm above: a `Device(i)` whose
        // `i` this graph has no entry for. `ImageGraph::declare_device` is
        // the only producer of such a value and always pushes first, so
        // this is unreachable by construction — kept and tested anyway,
        // because "index out of range" must never be a silent success.
        let programs = programs_map(program_with_init(
            "Blk",
            vec![cap_param("cap", "DeviceCap", "BlockHw")],
        ));
        let mut g = ImageGraph::default();
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg(
                "device",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Device(7)),
            )],
        });
        let err = check_init_args(&g, &programs).expect_err("device#7 does not exist");
        assert!(
            err.message.contains("which this image does not declare"),
            "{}",
            err.message
        );
    }

    #[test]
    fn an_actor_binding_mints_no_capability_at_all() {
        let programs = programs_map(program_with_init(
            "Store",
            vec![cap_param("cap", "DeviceCap", "BlockHw")],
        ));
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![],
        });
        let err = check_init_args(&g, &programs).expect_err("`img.actor` binds no device");
        assert!(
            err.message.contains("`img.actor(...)` binds no device"),
            "{}",
            err.message
        );
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
