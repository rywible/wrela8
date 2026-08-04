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

fn check_cores(graph: &ImageGraph) -> Result<(), SemaError> {
    if graph.cores < 1 {
        return Err(build_error(format!(
            "`Image` sealed with cores={} — cores must be a comptime usize ≥ 1 (05-library.md §9)",
            graph.cores
        )));
    }
    if graph.cores > wrela_machine::CORE_SLOTS {
        return Err(build_error(format!(
            "`Image(..., cores={})` exceeds CORE_SLOTS ({}) — soft page-packing ceiling, not a \
             published machine maximum",
            graph.cores,
            wrela_machine::CORE_SLOTS
        )));
    }
    Ok(())
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

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedImage {
    pub renderer_configs: crate::pixels::config::RendererConfigs,
}

fn pixels_diagnostic(diagnostic: crate::pixels::diagnostics::PixelsDiagnostic) -> SemaError {
    let mut extra_lines: Vec<String> = diagnostic
        .notes
        .iter()
        .map(|note| format!("note: {note}"))
        .collect();
    extra_lines.extend(diagnostic.help.iter().map(|help| format!("help: {help}")));
    SemaError {
        category: diagnostic.category(),
        message: diagnostic.message,
        line: diagnostic.primary.line,
        col: diagnostic.primary.col,
        extra_lines,
        omit_location: diagnostic.primary == Default::default(),
        missing_method: None,
    }
}

pub fn pixels_error(error: crate::pixels::diagnostics::PixelsError) -> SemaError {
    pixels_diagnostic(error.diagnostic().clone())
}

pub fn check_sealed(
    graph: &ImageGraph,
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<CheckedImage, SemaError> {
    check_cores(graph)?;
    check_construction_dag(graph)?;
    let mut declared: BTreeSet<String> = owner.declared_pools.clone();
    for p in programs.values() {
        declared.extend(p.declared_pools.iter().cloned());
    }
    check_pools_bound(graph, &declared)?;
    check_pool_decls(graph, programs)?;
    check_device_bound_once(graph)?;
    check_init_args(graph, programs)?;
    check_failure_policy(graph)?;
    check_blk_device_decls(graph, programs)?;
    check_blk_config_names_the_blk_device(
        graph,
        programs,
        &pool_backings(graph, &closure_layouts(programs)?)?,
    )?;
    check_driver_mode(graph)?;
    check_vector_bindings(graph, programs)?;
    crate::placement::check_annotations(graph).map_err(build_error)?;
    let renderer_configs = crate::pixels::config::validate_renderers(owner, programs, graph)
        .map_err(|error| pixels_diagnostic(error.diagnostic().clone()))?;
    Ok(CheckedImage { renderer_configs })
}

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
        let vector = device_vector(&decl.args).or_else(|| {
            device_index_of_driver(decl)
                .and_then(|i| graph.devices.get(i).and_then(|d| device_vector(&d.args)))
        });
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

pub fn blk_capacity_sectors(graph: &ImageGraph) -> Option<u64> {
    for d in &graph.devices {
        if let Some(a) = d.args.iter().find(|a| a.label == "capacity_sectors") {
            return int_value_as_i128(&a.value).and_then(|v| u64::try_from(v).ok());
        }
    }
    None
}

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

fn check_blk_device_decls(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
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

fn check_blk_config_names_the_blk_device(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
    backings: &BTreeMap<String, PoolBacking>,
) -> Result<(), SemaError> {
    let mut configured: Option<&str> = None;
    for p in programs.values() {
        for (pool_name, _depth) in &p.virtqueue_configures {
            if configured.is_some() {
                return Ok(());
            }
            configured = Some(pool_name.as_str());
        }
    }
    let Some(pool_name) = configured else {
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
    for (i, d) in graph.renderers.iter().enumerate() {
        out.push((ImageDeclRef::Renderer(i), d.args.as_slice()));
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

pub const MAX_POOL_BYTES: u64 = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolBacking {
    pub name: String,
    pub is_dma: bool,
    pub payload: String,
    pub slots: u64,
    pub slot_bytes: u64,
    pub bytes: u64,
    pub align: u64,
    pub device: Option<usize>,
}

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
    match widest {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        _ => 8,
    }
}

pub(crate) fn closure_layouts(
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<BTreeMap<String, types::LayoutType>, SemaError> {
    let mut out: BTreeMap<String, types::LayoutType> = BTreeMap::new();
    let mut from: BTreeMap<String, String> = BTreeMap::new();
    for (module, p) in programs {
        for l in &p.layouts {
            if !p.structs.contains_key(&l.name) {
                continue;
            }
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
                bytes: 0,
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
        for e in p.enums.values() {
            for payloads in &e.variant_payload_types {
                for t in payloads {
                    own_handles_in_type(t, &mut out);
                }
            }
            for f in e.methods.values() {
                own_handles_in_fn(f, &mut out);
            }
            for f in e.assoc_fns.values() {
                own_handles_in_fn(f, &mut out);
            }
        }
        for inst in p.instantiations.values() {
            match inst {
                TypedInstantiation::Fn(f) => own_handles_in_fn(f, &mut out),
                TypedInstantiation::Struct(s) => own_handles_in_struct(s, &mut out),
                TypedInstantiation::Enum(_) => {}
            }
        }
    }
    out
}

pub fn check_pool_decls(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    let backings = pool_backings(graph, &closure_layouts(programs)?)?;
    for (pool, payload) in own_handles_in_closure(programs) {
        let Some(b) = backings.get(&pool) else {
            continue;
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

#[derive(Clone, Copy)]
enum DeclKind {
    Driver,
    Actor,
}

fn reserved_args(kind: DeclKind) -> &'static [&'static str] {
    match kind {
        DeclKind::Driver => &["device", "vector", "core", "mailbox"],
        DeclKind::Actor => &["mailbox", "core"],
    }
}

pub(crate) fn is_reserved_actor_arg(label: &str) -> bool {
    reserved_args(DeclKind::Actor).contains(&label)
}

const CAPABILITY_TYPES: &[(&str, usize)] = &[
    ("DeviceCap", 1),
    ("DmaPool", 2),
    ("DmaShared", 2),
    ("IrqCap", 1),
    ("Mmio", 1),
];

pub(crate) fn is_capability_type_name(name: &str) -> bool {
    capability_generic_arity(name).is_some()
}

pub(crate) fn capability_generic_arity(name: &str) -> Option<usize> {
    CAPABILITY_TYPES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, arity)| *arity)
}

const PROTOCOL_STATE_TYPES: &[&str] = &[
    "ResetDevice",
    "AcknowledgedDevice",
    "DriverClaimedDevice",
    "FeaturesNegotiatedDevice",
    "FeaturesAcceptedDevice",
    "QueuesConfiguredDevice",
    "RunningDevice",
];

pub(crate) fn is_protocol_state_type_name(name: &str) -> bool {
    PROTOCOL_STATE_TYPES.contains(&name)
}

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

fn value_fits_param_type(arg: &DeclArg, param_ty: &Type) -> bool {
    if &arg.ty == param_ty {
        return true;
    }
    match (
        int_value_as_i128(&arg.value),
        crate::eval::value::int_bounds(param_ty),
    ) {
        (Some(v), Some((lo, hi))) => v >= lo && v <= hi,
        _ => false,
    }
}

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

fn resolved_struct<'a>(
    programs: &'a BTreeMap<String, TypedProgram>,
    visible_name: &str,
) -> Option<&'a crate::sema::typed::TypedStruct> {
    let mut resolved = programs.values().filter_map(|program| {
        let module = program.type_decl_modules.get(visible_name)?;
        let declaration = program.type_decl_names.get(visible_name)?;
        programs
            .get(module)
            .or_else(|| {
                let short = module.strip_prefix("core.").or_else(|| {
                    module
                        .strip_prefix("drivers.")
                        .filter(|short| *short == "display")
                })?;
                programs.get(short)
            })
            .and_then(|program| program.structs.get(declaration))
    });
    let first = resolved.next();
    if first.is_some() && resolved.next().is_none() {
        return first;
    }
    let mut exact = programs
        .values()
        .filter_map(|program| program.structs.get(visible_name));
    let first = exact.next();
    (exact.next().is_none()).then_some(first).flatten()
}

fn nominal_ids(
    programs: &BTreeMap<String, TypedProgram>,
    visible_name: &str,
) -> BTreeSet<(String, String)> {
    let mut ids = BTreeSet::new();
    for (module, program) in programs {
        if let (Some(declaring_module), Some(declaration)) = (
            program.type_decl_modules.get(visible_name),
            program.type_decl_names.get(visible_name),
        ) {
            ids.insert((declaring_module.clone(), declaration.clone()));
        }
        if program.structs.contains_key(visible_name) || program.enums.contains_key(visible_name) {
            ids.insert((module.clone(), visible_name.to_string()));
        }
    }
    ids
}

fn same_nominal_type(programs: &BTreeMap<String, TypedProgram>, left: &Type, right: &Type) -> bool {
    if left == right {
        return true;
    }
    let (Type::Named(left, left_args), Type::Named(right, right_args)) = (left, right) else {
        return false;
    };
    if left_args != right_args {
        return false;
    }
    let left_ids = nominal_ids(programs, left);
    let right_ids = nominal_ids(programs, right);
    !left_ids.is_disjoint(&right_ids)
}

fn find_constructor(programs: &BTreeMap<String, TypedProgram>, actor_type: &Type) -> Constructor {
    let Type::Named(struct_name, targs) = actor_type else {
        return Constructor::Init(Vec::new());
    };
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
    if let Some(f) = resolved_struct(programs, struct_name).and_then(|strukt| strukt.init.as_ref())
    {
        return Constructor::Init(
            f.params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect(),
        );
    }
    let Some(s) = resolved_struct(programs, struct_name) else {
        return Constructor::Init(Vec::new());
    };
    Constructor::Fields(
        s.fields
            .iter()
            .filter_map(|name| s.field_types.get(name).map(|ty| (name.clone(), ty.clone())))
            .collect(),
    )
}

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
    programs: &BTreeMap<String, TypedProgram>,
    device_caps_seen: &mut usize,
) -> Result<(), SemaError> {
    let rendered = types::render_type(param_ty);
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
        let owner = match cap_name {
            "Mmio" => {
                "an `Mmio[L]` is not minted at the image binding at all: the sealed transport \
                 hands one out from an already-claimed device \
                 (`claimed.map_partition(L)` — 03-hardware.md §2/§9). Declare it as a `@driver` \
                 field and assign it inside `init`, which is where 03 §1's own worked \
                 constructor puts it"
            }
            "IrqCap" => {
                return check_irq_cap_mint(
                    decl_ref,
                    struct_name,
                    param_name,
                    param_ty,
                    args,
                    graph,
                );
            }
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
    let bound = types::render_type(&device.device_type);
    let declared_type = match param_ty {
        Type::Named(_, targs) => match targs.first() {
            Some(crate::sema::types::TypeArg::Type(ty)) => ty,
            _ => &device.device_type,
        },
        _ => &device.device_type,
    };
    if !same_nominal_type(programs, declared_type, &device.device_type) {
        let declared = types::render_type(declared_type);
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
        return Ok(());
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
    let Constructor::Init(params) = &ctor else {
        return Ok(());
    };
    let mut device_caps_seen = 0usize;
    for (name, ty) in params {
        if satisfied.contains(name) {
            continue;
        }
        if resolved_struct(programs, struct_name)
            .and_then(|strukt| strukt.init.as_ref())
            .and_then(|init| init.params.iter().find(|param| param.name == *name))
            .is_some_and(|param| param.default.is_some())
        {
            continue;
        }
        if let Type::Named(tn, _) = ty {
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
                    programs,
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

pub const DEADLINE_VECTOR: u64 = 0;

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

struct IrqBindSite {
    driver: String,
    vector: u64,
    handler: String,
    site: String,
}

pub fn check_vector_bindings(
    graph: &ImageGraph,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<(), SemaError> {
    let mut claimed: BTreeMap<u64, String> = BTreeMap::new();
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
        if let Some(prev) = claimed.insert(v, format!("device#{i}")) {
            return Err(build_error(format!(
                "`device#{i}` and `{prev}` both declare `vector={v}` — 03-hardware.md §6: \
                 exactly one handler per vector, and a vector is owned by exactly one device"
            )));
        }
    }

    let mut binds: Vec<IrqBindSite> = Vec::new();
    let mut take_irq_sites: Vec<(String, Option<u64>, String)> = Vec::new();
    for (di, decl) in graph.drivers.iter().enumerate() {
        let Type::Named(driver, targs) = &decl.actor_type else {
            continue;
        };
        let device_index = device_index_of_driver(decl);
        let bound_device_vector =
            device_index.and_then(|i| graph.devices.get(i).and_then(|d| device_vector(&d.args)));
        let direct_vector = device_vector(&decl.args);
        if let (Some(direct), Some(device)) = (direct_vector, bound_device_vector)
            && direct != device
        {
            return Err(build_error(format!(
                "`driver#{di}` declares `vector={direct}` but its bound device declares \
                 `vector={device}`"
            )));
        }
        if let Some(vector) = direct_vector {
            if vector == DEADLINE_VECTOR || vector > 63 {
                return Err(build_error(format!(
                    "`driver#{di}` declares `vector={vector}`, but a device-owned vector must \
                     be in 1..=63"
                )));
            }
            let owner = device_index
                .map(|index| format!("device#{index}"))
                .unwrap_or_else(|| format!("driver#{di}"));
            if let Some(previous) = claimed.get(&vector) {
                if previous != &owner {
                    return Err(build_error(format!(
                        "`driver#{di}` and `{previous}` both declare `vector={vector}` — \
                         exactly one device may own a vector"
                    )));
                }
            } else {
                claimed.insert(vector, owner);
            }
        }
        let vector = direct_vector.or(bound_device_vector);
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
        return device_vector(&decl.args).or_else(|| {
            device_index_of_driver(decl)
                .and_then(|i| graph.devices.get(i))
                .and_then(|d| device_vector(&d.args))
        });
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
        TypedStmtKind::While { cond, body, .. } => {
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
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::CallValue(f, args) => {
            collect_irq_ops_expr(f, driver, vector, site, binds, take_irq);
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Try(inner, _) => {
            collect_irq_ops_expr(inner, driver, vector, site, binds, take_irq);
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                collect_irq_ops_expr(a, driver, vector, site, binds, take_irq);
            }
        }
        TypedExprKind::Tuple(args) | TypedExprKind::List(args) => {
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

pub fn check_failure_policy(graph: &ImageGraph) -> Result<(), SemaError> {
    match graph.on_failures.len() {
        0 => Err(build_error(
            "image has no `img.on_failure(policy=...)` failure policy".to_string(),
        )),
        1 => Ok(()),
        _ => Err(build_error(
            "image declares `img.on_failure` more than once".to_string(),
        )),
    }
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
            span: Default::default(),
        }
    }

    #[test]
    fn cores_default_passes_seal_bounds() {
        let g = ImageGraph::default();
        assert_eq!(g.cores, 1);
        assert!(check_cores(&g).is_ok());
    }

    #[test]
    fn cores_above_slots_names_the_packing_ceiling() {
        let mut g = ImageGraph::default();
        g.cores = wrela_machine::CORE_SLOTS + 1;
        let err = check_cores(&g).expect_err("N > CORE_SLOTS must fail");
        assert_eq!(err.category, "build");
        assert!(err.message.contains("CORE_SLOTS"), "{}", err.message);
        assert!(
            err.message.contains("soft page-packing ceiling"),
            "{}",
            err.message
        );
        assert!(
            !err.message.contains("machine has"),
            "must not claim a published machine max: {}",
            err.message
        );
    }

    #[test]
    fn cores_zero_is_rejected_at_seal() {
        let mut g = ImageGraph::default();
        g.cores = 0;
        let err = check_cores(&g).expect_err("cores=0 must fail at seal");
        assert!(
            err.message.contains("≥ 1") || err.message.contains(">= 1"),
            "{}",
            err.message
        );
    }

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

    #[test]
    fn a_renderer_declaration_cycle_is_rejected() {
        let mut g = ImageGraph::default();
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Consumer".to_string(), vec![]),
            args: vec![decl_arg(
                "renderer",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Renderer(0)),
            )],
        });
        g.renderers.push(crate::eval::image::RendererDecl {
            params_type: Type::Named("Params".to_string(), vec![]),
            actor_type: Type::Named("Renderer".to_string(), vec![]),
            args: vec![decl_arg(
                "consumer",
                Type::Named("ImageDecl".to_string(), vec![]),
                Value::ImageDecl(ImageDeclRef::Actor(0)),
            )],
            span: Default::default(),
        });
        let err =
            check_construction_dag(&g).expect_err("renderer#0 <-> actor#0 is a declaration cycle");
        assert_eq!(err.category, "build");
        assert!(err.message.contains("cycle"));
        assert!(
            err.extra_lines
                .iter()
                .any(|line| line.contains("renderer#0")),
            "cycle why-chain must name the renderer hop: {:?}",
            err.extra_lines
        );
    }

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

    #[test]
    fn the_reachable_pool_arms_no_golden_spells() {
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);

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
        let span = crate::syntax::ast::Span {
            line: 1,
            col: 1,
            ..Default::default()
        };

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
        let g_no_dev = driver_graph(Some("BlockHw"), false, vec![]);
        let err =
            check_init_args(&g_no_dev, &programs).expect_err("IrqCap init param needs device=");
        assert!(
            err.message.contains("declares no `device=`"),
            "IrqCap no-device: {}",
            err.message
        );
    }

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
        let span = crate::syntax::ast::Span {
            line: 1,
            col: 1,
            ..Default::default()
        };
        let layouts = layouts_of(vec![dma_layout("Hdr", &[("a", 4)])]);
        let dma_args = vec![
            handle_arg("device", ImageDeclRef::Device(0)),
            decl_arg("count", Type::I64, Value::I64(2)),
        ];

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
        assert!(!err.message.contains(".init"));
    }

    #[test]
    fn a_declared_init_still_wins_over_the_fields_of_the_same_struct() {
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

    #[test]
    fn a_missing_failure_policy_is_rejected() {
        let g = ImageGraph::default();
        let err = check_failure_policy(&g).expect_err("no on_failure");
        assert!(err.message.contains("no `img.on_failure"));
    }

    #[test]
    fn a_second_failure_policy_is_rejected() {
        let mut g = ImageGraph::default();
        for _ in 0..2 {
            g.on_failures.push(crate::eval::image::OnFailureDecl {
                args: vec![decl_arg(
                    "policy",
                    Type::Named("Failure".to_string(), vec![]),
                    Value::Enum(1, vec![]),
                )],
            });
        }
        let err = check_failure_policy(&g).expect_err("two on_failure");
        assert!(err.message.contains("more than once"));
    }

    #[test]
    fn a_single_failure_policy_is_accepted() {
        let mut g = ImageGraph::default();
        g.on_failures.push(crate::eval::image::OnFailureDecl {
            args: vec![decl_arg(
                "policy",
                Type::Named("Failure".to_string(), vec![]),
                Value::Enum(1, vec![]),
            )],
        });
        assert!(check_failure_policy(&g).is_ok());
    }
}
