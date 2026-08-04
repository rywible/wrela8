use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::sema::SemaError;
use crate::sema::typed::{
    TypedExpr, TypedExprKind, TypedFn, TypedInstantiation, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::syntax::ast::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Legal,
    Illegal { path: Vec<String>, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Legality {
    pub verdicts: BTreeMap<String, Verdict>,
}

impl Legality {
    pub fn verdict(&self, key: &str) -> Verdict {
        self.verdicts.get(key).cloned().unwrap_or(Verdict::Legal)
    }
}

pub fn require_legal(
    legality: &Legality,
    key: &str,
    context: &str,
    span: Span,
) -> Result<(), SemaError> {
    match legality.verdict(key) {
        Verdict::Legal => Ok(()),
        Verdict::Illegal { path, reason } => {
            let mut extra_lines = Vec::new();
            for hop in path.windows(2) {
                extra_lines.push(format!("  `{}` calls `{}`", hop[0], hop[1]));
            }
            let last = path.last().cloned().unwrap_or_else(|| key.to_string());
            extra_lines.push(format!("  `{last}` uses {reason}"));
            Err(SemaError {
                category: "comptime",
                message: format!(
                    "`{context}` requires a comptime-legal closure, but `{key}` is not \
                     comptime-callable"
                ),
                line: span.line,
                col: span.col,
                extra_lines,
                omit_location: false,
                missing_method: None,
            })
        }
    }
}

pub struct StandaloneScan {
    pub callees: BTreeSet<String>,
    pub illegal: Option<String>,
}

pub fn scan_standalone(expr: &TypedExpr) -> StandaloneScan {
    let mut scan = BodyScan::default();
    scan_expr(expr, &mut scan);
    StandaloneScan {
        callees: scan.callees,
        illegal: scan.illegal,
    }
}

pub fn direct_callees_of_body(body: &[TypedStmt]) -> BTreeSet<String> {
    let mut scan = BodyScan::default();
    scan_stmts(body, &mut scan);
    scan.callees
}

pub fn classify(program: &TypedProgram) -> Legality {
    let mut nodes = build_nodes(program);
    if let Some(image_fn) = &program.image_fn {
        if let Some(info) = nodes.get_mut(image_fn) {
            info.illegal = None;
        }
    }
    let mut verdicts: BTreeMap<String, Verdict> = BTreeMap::new();
    for (key, info) in &nodes {
        let verdict = match &info.illegal {
            Some(reason) => Verdict::Illegal {
                path: vec![info.display_name.clone()],
                reason: reason.clone(),
            },
            None => Verdict::Legal,
        };
        verdicts.insert(key.clone(), verdict);
    }

    loop {
        let snapshot = verdicts.clone();
        let mut changed = false;
        for (key, info) in &nodes {
            if matches!(snapshot.get(key), Some(Verdict::Illegal { .. })) {
                continue;
            }
            for callee in &info.callees {
                if let Some(Verdict::Illegal { path, reason }) = snapshot.get(callee) {
                    let mut full_path = vec![info.display_name.clone()];
                    full_path.extend(path.iter().cloned());
                    verdicts.insert(
                        key.clone(),
                        Verdict::Illegal {
                            path: full_path,
                            reason: reason.clone(),
                        },
                    );
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    Legality { verdicts }
}

pub type Authority = crate::sema::types::CapabilityAuthority;

pub fn check_provenance(program: &TypedProgram, authority: &Authority) -> Result<(), SemaError> {
    let nodes = build_nodes(program);
    let touches = hardware_touches(program, authority);
    if touches.is_empty() {
        return Ok(());
    }

    let mut authorized: BTreeSet<String> = authority
        .roots
        .iter()
        .filter(|k| nodes.contains_key(*k))
        .cloned()
        .collect();
    let driver_root_names: BTreeSet<&str> = authority
        .roots
        .iter()
        .filter_map(|r| r.split('.').next())
        .collect();
    for key in nodes.keys() {
        let Some(rest) = key.strip_prefix("struct:") else {
            continue;
        };
        let Some(dot) = rest.rfind('.') else {
            continue;
        };
        let type_part = &rest[..dot];
        let bare = type_part.split('[').next().unwrap_or(type_part);
        if driver_root_names.contains(bare) {
            authorized.insert(key.clone());
        }
    }
    loop {
        let mut changed = false;
        for (key, info) in &nodes {
            if !authorized.contains(key) {
                continue;
            }
            for callee in &info.callees {
                if authorized.insert(callee.clone()) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for (key, reason) in &touches {
        if authorized.contains(key) {
            continue;
        }
        let span = authority.spans.get(key).copied();
        return Err(SemaError {
            category: "type",
            message: format!(
                "`{key}` touches hardware state ({reason}) but is not reachable through any \
                 `@driver`'s authority in this module — 03-hardware.md §1: a function that \
                 touches MMIO, DMA, or IRQ state must be reachable through the owning driver's \
                 authority"
            ),
            line: span.map(|s| s.line).unwrap_or(0),
            col: span.map(|s| s.col).unwrap_or(0),
            extra_lines: Vec::new(),
            omit_location: span.is_none(),
            missing_method: None,
        });
    }
    Ok(())
}

fn hardware_touches(program: &TypedProgram, authority: &Authority) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, f) in &program.fns {
        note_touch(&mut out, name.clone(), f, authority);
    }
    for (struct_name, s) in &program.structs {
        for (member, f) in &s.methods {
            note_touch(&mut out, format!("{struct_name}.{member}"), f, authority);
        }
        for (member, f) in &s.assoc_fns {
            note_touch(&mut out, format!("{struct_name}.{member}"), f, authority);
        }
        if let Some(f) = &s.init {
            note_touch(&mut out, format!("{struct_name}.init"), f, authority);
        }
    }
    for (key, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => note_touch(&mut out, key.clone(), f, authority),
            TypedInstantiation::Struct(s) => {
                for (member, f) in &s.methods {
                    note_touch(&mut out, format!("{key}.{member}"), f, authority);
                }
                for (member, f) in &s.assoc_fns {
                    note_touch(&mut out, format!("{key}.{member}"), f, authority);
                }
                if let Some(f) = &s.init {
                    note_touch(&mut out, format!("{key}.init"), f, authority);
                }
            }
            TypedInstantiation::Enum(_) => {}
        }
    }
    out
}

fn note_touch(out: &mut BTreeMap<String, String>, key: String, f: &TypedFn, authority: &Authority) {
    for p in &f.params {
        if let Some(found) = capability_in_type(&p.ty, authority) {
            out.insert(key, format!("its own `{}: {found}`", p.name));
            return;
        }
    }
    let mut scan = BodyScan::default();
    scan_hardware_stmts(&f.body, authority, &mut scan);
    if let Some(reason) = scan.illegal {
        out.insert(key, reason);
    }
}

fn capability_in_type(ty: &crate::sema::types::Type, authority: &Authority) -> Option<String> {
    use crate::sema::types::{Type, TypeArg, render_type};
    match ty {
        Type::Named(name, _)
            if crate::eval::image_checks::is_capability_type_name(name)
                || authority.capability_bearing.contains(name) =>
        {
            Some(render_type(ty))
        }
        Type::Named(name, _) if name == "Actor" => None,
        Type::Array(elem, _) => capability_in_type(elem, authority),
        Type::Tuple(elems) => elems.iter().find_map(|e| capability_in_type(e, authority)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            capability_in_type(inner, authority)
        }
        Type::Result(ok, err) => {
            capability_in_type(ok, authority).or_else(|| capability_in_type(err, authority))
        }
        Type::Fn(params, ret) => params
            .iter()
            .find_map(|(_, t)| capability_in_type(t, authority))
            .or_else(|| capability_in_type(ret, authority)),
        Type::Named(_, targs) => targs.iter().find_map(|a| match a {
            TypeArg::Type(t) => capability_in_type(t, authority),
            _ => None,
        }),
        _ => None,
    }
}

fn expr_hardware_reason(e: &TypedExpr, authority: &Authority) -> Option<String> {
    let by_type = capability_in_type(&e.ty, authority).map(|found| format!("a `{found}` value"));
    match &e.kind {
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_mmio_access_intrinsic(key) =>
        {
            Some(if key == "Mmio.read" {
                "an MMIO register read".to_string()
            } else {
                "an MMIO register write".to_string()
            })
        }
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_device_transport_intrinsic(key) =>
        {
            Some(match key.as_str() {
                "Device.claim" => "a device claim (03-hardware.md §9's bring-up chain)".to_string(),
                "Device.take_irq" => {
                    "taking an `IrqCap` from a claimed device (03-hardware.md §6)".to_string()
                }
                _ => "an MMIO claim partitioning".to_string(),
            })
        }
        TypedExprKind::Intrinsic { key, .. } if crate::sema::bodies::is_irq_cap_intrinsic(key) => {
            Some(if key == "IrqCap.bind" {
                "binding an interrupt vector (03-hardware.md §6)".to_string()
            } else {
                "unmasking an interrupt vector (03-hardware.md §6)".to_string()
            })
        }
        TypedExprKind::Intrinsic { key, .. } if crate::sema::bodies::is_queue_op_intrinsic(key) => {
            Some(if key == "VirtQueue.reserve" {
                "a proven queue reservation (03-hardware.md §4)".to_string()
            } else {
                "a prepared block operation (03-hardware.md §4)".to_string()
            })
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
        | TypedExprKind::Field(..)
        | TypedExprKind::Index(..)
        | TypedExprKind::Call { .. }
        | TypedExprKind::CallValue(..)
        | TypedExprKind::ToScalar(_)
        | TypedExprKind::Neg(_)
        | TypedExprKind::BitNot(_)
        | TypedExprKind::Take(_)
        | TypedExprKind::Try(..)
        | TypedExprKind::Binary(..)
        | TypedExprKind::OpCall(..)
        | TypedExprKind::Is(..)
        | TypedExprKind::Not(_)
        | TypedExprKind::And(..)
        | TypedExprKind::Or(..)
        | TypedExprKind::EnumConstruct { .. }
        | TypedExprKind::Closure { .. }
        | TypedExprKind::Tuple(_)
        | TypedExprKind::List(_)
        | TypedExprKind::StructLiteral { .. }
        | TypedExprKind::Panic(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::Intrinsic { .. }
        | TypedExprKind::Await(_)
        | TypedExprKind::Send(_)
        | TypedExprKind::GroupChild(_) => by_type,
    }
}

struct NodeInfo {
    display_name: String,
    callees: BTreeSet<String>,
    illegal: Option<String>,
}

fn build_nodes(program: &TypedProgram) -> BTreeMap<String, NodeInfo> {
    let mut nodes = BTreeMap::new();

    for (name, f) in &program.fns {
        insert_fn_node(&mut nodes, name.clone(), f);
    }

    for (name, f) in &program.imported.fns {
        if !nodes.contains_key(name) {
            insert_fn_node(&mut nodes, name.clone(), f);
        }
    }

    for (struct_name, s) in &program.structs {
        for (member, f) in &s.methods {
            insert_fn_node(&mut nodes, format!("{struct_name}.{member}"), f);
        }
        for (member, f) in &s.assoc_fns {
            insert_fn_node(&mut nodes, format!("{struct_name}.{member}"), f);
        }
        if let Some(f) = &s.init {
            insert_fn_node(&mut nodes, format!("{struct_name}.init"), f);
        }
    }

    for (struct_name, s) in &program.imported.structs {
        for (member, f) in &s.methods {
            let key = format!("{struct_name}.{member}");
            if !nodes.contains_key(&key) {
                insert_fn_node(&mut nodes, key, f);
            }
        }
        for (member, f) in &s.assoc_fns {
            let key = format!("{struct_name}.{member}");
            if !nodes.contains_key(&key) {
                insert_fn_node(&mut nodes, key, f);
            }
        }
        if let Some(f) = &s.init {
            let key = format!("{struct_name}.init");
            if !nodes.contains_key(&key) {
                insert_fn_node(&mut nodes, key, f);
            }
        }
    }

    for (enum_name, e) in &program.enums {
        for (member, f) in &e.methods {
            insert_fn_node(&mut nodes, format!("{enum_name}.{member}"), f);
        }
        for (member, f) in &e.assoc_fns {
            insert_fn_node(&mut nodes, format!("{enum_name}.{member}"), f);
        }
    }

    for (key, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => insert_fn_node(&mut nodes, key.clone(), f),
            TypedInstantiation::Struct(s) => {
                for (member, f) in &s.methods {
                    insert_fn_node(&mut nodes, format!("{key}.{member}"), f);
                }
                for (member, f) in &s.assoc_fns {
                    insert_fn_node(&mut nodes, format!("{key}.{member}"), f);
                }
                if let Some(f) = &s.init {
                    insert_fn_node(&mut nodes, format!("{key}.init"), f);
                }
            }
            TypedInstantiation::Enum(_) => {}
        }
    }

    nodes
}

fn insert_fn_node(nodes: &mut BTreeMap<String, NodeInfo>, key: String, f: &TypedFn) {
    let mut scan = BodyScan::default();
    scan_stmts(&f.body, &mut scan);
    for p in &f.params {
        if let Some(def) = &p.default {
            scan_expr(def, &mut scan);
        }
    }
    if f.is_async {
        scan.note_illegal("an `async fn`");
    }
    nodes.insert(
        key.clone(),
        NodeInfo {
            display_name: key,
            callees: scan.callees,
            illegal: scan.illegal,
        },
    );
}

#[derive(Default)]
struct BodyScan {
    callees: BTreeSet<String>,
    illegal: Option<String>,
}

impl BodyScan {
    fn note_illegal(&mut self, reason: &str) {
        if self.illegal.is_none() {
            self.illegal = Some(reason.to_string());
        }
    }
}

fn is_float_type(ty: &crate::sema::types::Type) -> bool {
    matches!(
        ty,
        crate::sema::types::Type::F32 | crate::sema::types::Type::F64
    )
}

fn dma_touch_reason(ty: &crate::sema::types::Type) -> Option<String> {
    use crate::sema::types::Type;
    match ty {
        Type::Own(pool, inner) => Some(format!(
            "own[{pool}] {}",
            crate::sema::types::render_type(inner)
        )),
        Type::Named(name, _) if name == "DmaShared" => Some(crate::sema::types::render_type(ty)),
        Type::Option(inner) | Type::Static(inner) => dma_touch_reason(inner),
        Type::Array(elem, _) => dma_touch_reason(elem),
        Type::Tuple(elems) => elems.iter().find_map(dma_touch_reason),
        Type::Result(ok, err) => dma_touch_reason(ok).or_else(|| dma_touch_reason(err)),
        _ => None,
    }
}

fn is_plain_self_field_channel(target: &TypedExpr) -> bool {
    let TypedExprKind::Field(base, _) = &target.kind else {
        return false;
    };
    let TypedExprKind::Local(name) = &base.kind else {
        return false;
    };
    if name != "self" {
        return false;
    }
    !crate::sema::bodies::is_interrupt_cell_type(&target.ty)
}

fn is_format_method_callee(callee: &crate::sema::typed::CalleeKey) -> bool {
    match callee {
        crate::sema::typed::CalleeKey::Method(_, m)
        | crate::sema::typed::CalleeKey::MethodInstance(_, m) => m == "format",
        _ => false,
    }
}

use super::walk::{self, Visitor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectContext {
    Comptime,
    Isr,
    BottomHalf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectClass {
    Await,
    Send,
    WithGroup,
    BareSend,
    MmioAccess,
    DeviceTransport,
    QueueOp,
    IrqCapOp,
    RestrictedIntrinsic,
    Now,
    Entropy,
    GroupOp,
    Float,
    Format,
    UnboundedLoop,
    PlainSelfFieldChannel,
    DmaPayload,
}

fn effect_reason(effect: EffectClass, ctx: EffectContext) -> Option<&'static str> {
    use EffectClass::*;
    use EffectContext::*;
    match (effect, ctx) {
        (Await, Comptime) => Some("an `await` expression"),
        (Await, Isr) => Some("an `await`"),
        (Await, BottomHalf) => Some("an `await` (stays active while waiting)"),
        (Send, Comptime) => Some("a `send` expression"),
        (Send, Isr) => Some("a `send` (call another actor)"),
        (Send, BottomHalf) => Some("a `send` (call another actor)"),
        (WithGroup, Comptime) => Some("a `with group` block"),
        (WithGroup, Isr) => Some("a `with group` (call another actor / block)"),
        (WithGroup, BottomHalf) => Some("a `with group` (stays active while waiting)"),
        (BareSend, Comptime) => Some("a `send` statement"),
        (BareSend, Isr) => Some("a bare `send` (call another actor)"),
        (BareSend, BottomHalf) => Some("a bare `send` (call another actor)"),
        (MmioAccess, Comptime) => Some("a volatile MMIO register access"),
        (MmioAccess, Isr | BottomHalf) => None,
        (DeviceTransport, Comptime) => Some("a device bring-up transition (03-hardware.md §9)"),
        (DeviceTransport, Isr) => Some("a device bring-up transition (not in the ISR effect set)"),
        (DeviceTransport, BottomHalf) => None,
        (QueueOp, Comptime) => Some("a queue operation (03-hardware.md §4)"),
        (QueueOp, Isr) => {
            Some("a queue operation (not in the ISR effect set — drain belongs in `@task`)")
        }
        (QueueOp, BottomHalf) => None,
        (IrqCapOp, Comptime) => Some("an interrupt-vector operation (03-hardware.md §6)"),
        (IrqCapOp, Isr) => Some("an interrupt-vector bind/unmask (not in the ISR effect set)"),
        (IrqCapOp, BottomHalf) => None,
        (RestrictedIntrinsic, Comptime) => Some("an `@image` builder intrinsic"),
        (RestrictedIntrinsic, Isr | BottomHalf) => None,
        (Now, Comptime) => Some("`now()` (a runtime-only clock read)"),
        (Now, Isr) => Some("a runtime clock read (`now()` — not in the ISR effect set)"),
        (Now, BottomHalf) => None,
        (Entropy, Comptime) => Some("`entropy[N]()` (a runtime-only entropy fill)"),
        (Entropy, Isr) => {
            Some("a runtime entropy fill (`entropy[N]()` — not in the ISR effect set)")
        }
        (Entropy, BottomHalf) => None,
        (GroupOp, Comptime) => Some("a `group` construct"),
        (GroupOp, Isr) => Some("a `group` construct (call another actor / block)"),
        (GroupOp, BottomHalf) => None,
        (Float, Isr) => Some("floating point"),
        (Float, Comptime | BottomHalf) => None,
        (Format, Isr) => Some("formatting (f-string / Format)"),
        (Format, Comptime | BottomHalf) => None,
        (UnboundedLoop, Isr) => {
            Some("a loop (drain unbounded work — loops belong in the bottom half)")
        }
        (UnboundedLoop, Comptime | BottomHalf) => None,
        (PlainSelfFieldChannel, Isr) => {
            Some("a plain field as an ISR channel (03-hardware.md §6: use `InterruptCell[T]`)")
        }
        (PlainSelfFieldChannel, Comptime | BottomHalf) => None,
        (DmaPayload, Isr) => Some("a device-owned DMA payload (`own[P] T` / `DmaShared`)"),
        (DmaPayload, Comptime | BottomHalf) => None,
    }
}

fn stmt_kind_effects(kind: &TypedStmtKind) -> Vec<EffectClass> {
    match kind {
        TypedStmtKind::BareSend { .. } => vec![EffectClass::BareSend],
        TypedStmtKind::WithGroup { .. } => vec![EffectClass::WithGroup],
        TypedStmtKind::While { .. } | TypedStmtKind::For { .. } => {
            vec![EffectClass::UnboundedLoop]
        }
        TypedStmtKind::Assign { target, .. } if is_plain_self_field_channel(target) => {
            vec![EffectClass::PlainSelfFieldChannel]
        }
        _ => Vec::new(),
    }
}

fn stmt_effects(stmt: &TypedStmt) -> Vec<EffectClass> {
    stmt_kind_effects(&stmt.kind)
}

#[cfg(test)]
fn stmt_isr_forbidden_reason(kind: &TypedStmtKind) -> Option<&'static str> {
    stmt_kind_effects(kind)
        .into_iter()
        .find_map(|e| effect_reason(e, EffectContext::Isr))
}

#[cfg(test)]
fn expr_isr_forbidden_reason(e: &TypedExpr) -> Option<&'static str> {
    expr_effects(e)
        .into_iter()
        .find_map(|ef| effect_reason(ef, EffectContext::Isr))
}

#[cfg(test)]
fn bottom_half_expr_forbidden_reason(e: &TypedExpr) -> Option<&'static str> {
    expr_effects(e)
        .into_iter()
        .find_map(|ef| effect_reason(ef, EffectContext::BottomHalf))
}

fn expr_effects(e: &TypedExpr) -> Vec<EffectClass> {
    let mut out = Vec::new();
    match &e.kind {
        TypedExprKind::Await(_) => out.push(EffectClass::Await),
        TypedExprKind::Send(_) => out.push(EffectClass::Send),
        TypedExprKind::Float(_) => out.push(EffectClass::Float),
        TypedExprKind::Binary(..) | TypedExprKind::OpCall(..) | TypedExprKind::Neg(_)
            if is_float_type(&e.ty) =>
        {
            out.push(EffectClass::Float);
        }
        TypedExprKind::Field(..) | TypedExprKind::Take(_) | TypedExprKind::Local(_)
            if dma_touch_reason(&e.ty).is_some() =>
        {
            out.push(EffectClass::DmaPayload);
        }
        TypedExprKind::Call { callee, .. } if is_format_method_callee(callee) => {
            out.push(EffectClass::Format);
        }
        TypedExprKind::Intrinsic { key, .. } => {
            if crate::sema::bodies::is_mmio_access_intrinsic(key) {
                out.push(EffectClass::MmioAccess);
            } else if crate::sema::bodies::is_device_transport_intrinsic(key) {
                out.push(EffectClass::DeviceTransport);
            } else if crate::sema::bodies::is_queue_op_intrinsic(key) {
                out.push(EffectClass::QueueOp);
            } else if crate::sema::bodies::is_irq_cap_intrinsic(key) {
                out.push(EffectClass::IrqCapOp);
            } else if crate::sema::typed::is_restricted_intrinsic(key) {
                out.push(EffectClass::RestrictedIntrinsic);
            } else if key == "now" {
                out.push(EffectClass::Now);
            } else if key == "entropy" {
                out.push(EffectClass::Entropy);
            } else if key.starts_with("Group.") {
                out.push(EffectClass::GroupOp);
            }
        }
        _ => {}
    }
    out
}

struct EffectVisitor<'a> {
    ctx: EffectContext,
    scan: &'a mut BodyScan,
    collect_callees: bool,
    patterns: bool,
}

impl Visitor for EffectVisitor<'_> {
    fn pre_stmt(&mut self, stmt: &TypedStmt) {
        for effect in stmt_effects(stmt) {
            if let Some(reason) = effect_reason(effect, self.ctx) {
                self.scan.note_illegal(reason);
            }
        }
    }
    fn pre_expr(&mut self, expr: &TypedExpr) {
        for effect in expr_effects(expr) {
            if let Some(reason) = effect_reason(effect, self.ctx) {
                self.scan.note_illegal(reason);
            }
        }
    }
    fn on_callee(&mut self, key: String) {
        if self.collect_callees {
            self.scan.callees.insert(key);
        }
    }
    fn walk_patterns(&self) -> bool {
        self.patterns
    }
}

fn scan_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Comptime,
        scan,
        collect_callees: true,
        patterns: true,
    };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_expr(e: &TypedExpr, scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Comptime,
        scan,
        collect_callees: true,
        patterns: true,
    };
    walk::walk_expr(e, &mut v);
}

struct HardwareVisitor<'a> {
    authority: &'a Authority,
    scan: &'a mut BodyScan,
}

impl Visitor for HardwareVisitor<'_> {
    fn pre_expr(&mut self, expr: &TypedExpr) {
        if let Some(reason) = expr_hardware_reason(expr, self.authority) {
            if self.scan.illegal.is_none() {
                self.scan.illegal = Some(reason);
            }
        }
    }
}

fn scan_hardware_stmts(stmts: &[TypedStmt], authority: &Authority, scan: &mut BodyScan) {
    let mut v = HardwareVisitor { authority, scan };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_bottom_half_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::BottomHalf,
        scan,
        collect_callees: false,
        patterns: false,
    };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_isr_forbidden_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Isr,
        scan,
        collect_callees: false,
        patterns: false,
    };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_isr_forbidden_expr(e: &TypedExpr, scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Isr,
        scan,
        collect_callees: false,
        patterns: false,
    };
    walk::walk_expr(e, &mut v);
}

struct IsrBindVisitor<'a> {
    roots: &'a mut BTreeSet<String>,
}

impl Visitor for IsrBindVisitor<'_> {
    fn pre_expr(&mut self, e: &TypedExpr) {
        if let TypedExprKind::Intrinsic { key, args, .. } = &e.kind {
            if key == "IrqCap.bind" {
                for (label, arg) in args {
                    if label == "handler" {
                        if let TypedExprKind::FnRef(k) = &arg.kind {
                            self.roots.insert(k.spelling());
                        }
                    }
                }
            }
        }
    }
}

fn scan_isr_bind_stmts(stmts: &[TypedStmt], _scan: &mut BodyScan, roots: &mut BTreeSet<String>) {
    let mut v = IsrBindVisitor { roots };
    walk::walk_stmts(stmts, &mut v);
}

fn task_method_keys(program: &TypedProgram) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (sname, s) in &program.structs {
        for (mname, f) in &s.methods {
            if f.is_task {
                keys.insert(format!("{sname}.{mname}"));
            }
        }
    }
    for (ikey, inst) in &program.instantiations {
        if let TypedInstantiation::Struct(s) = inst {
            for (mname, f) in &s.methods {
                if f.is_task {
                    keys.insert(format!("{ikey}.{mname}"));
                }
            }
        }
    }
    keys
}

pub fn check_isr_effects(program: &TypedProgram) -> Result<(), SemaError> {
    let roots = collect_isr_roots(program);
    if roots.is_empty() {
        return Ok(());
    }
    let nodes = build_nodes(program);
    let tasks = task_method_keys(program);
    let mut in_isr: BTreeSet<String> = roots
        .iter()
        .filter(|k| nodes.contains_key(*k))
        .cloned()
        .collect();
    loop {
        let mut changed = false;
        for (key, info) in &nodes {
            if !in_isr.contains(key) {
                continue;
            }
            for callee in &info.callees {
                if tasks.contains(callee) {
                    continue;
                }
                if in_isr.insert(callee.clone()) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut forbidden: BTreeMap<String, String> = BTreeMap::new();
    for key in &in_isr {
        if let Some(reason) = isr_forbidden_of(program, key) {
            forbidden.insert(key.clone(), reason);
        }
    }
    for (key, reason) in &forbidden {
        let path = isr_path_from_root(&roots, &nodes, key).unwrap_or_else(|| vec![key.clone()]);
        let path_text = path.join(" -> ");
        return Err(SemaError {
            category: "type",
            message: format!(
                "ISR `{path_text}` {reason} — 03-hardware.md §6: an interrupt handler's \
                 transitive effects are restricted to typed MMIO, `InterruptCell[T]`, \
                 helpers with that same set, and `wake` of a statically bound task"
            ),
            line: 0,
            col: 0,
            extra_lines: Vec::new(),
            omit_location: true,
            missing_method: None,
        });
    }
    Ok(())
}

pub fn check_wake_sites(program: &TypedProgram) -> Result<(), SemaError> {
    let task_keys = task_method_keys(program);
    let isr_keys = {
        let roots = collect_isr_roots(program);
        if roots.is_empty() {
            BTreeSet::new()
        } else {
            let nodes = build_nodes(program);
            let mut in_isr: BTreeSet<String> = roots
                .iter()
                .filter(|k| nodes.contains_key(*k))
                .cloned()
                .collect();
            loop {
                let mut changed = false;
                for (key, info) in &nodes {
                    if !in_isr.contains(key) {
                        continue;
                    }
                    for callee in &info.callees {
                        if task_keys.contains(callee) {
                            continue;
                        }
                        if in_isr.insert(callee.clone()) {
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            in_isr
        }
    };

    let mut bad: Option<String> = None;
    let mut note = |key: &str, f: &TypedFn| {
        if bad.is_some() {
            return;
        }
        if fn_contains_wake(f) && !isr_keys.contains(key) && !task_keys.contains(key) {
            bad = Some(key.to_string());
        }
    };
    for (name, f) in &program.fns {
        note(name, f);
    }
    for (sname, s) in &program.structs {
        if let Some(f) = &s.init {
            note(&format!("{sname}.init"), f);
        }
        for (m, f) in &s.methods {
            note(&format!("{sname}.{m}"), f);
        }
        for (m, f) in &s.assoc_fns {
            note(&format!("{sname}.{m}"), f);
        }
    }
    for (ikey, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => note(ikey, f),
            TypedInstantiation::Struct(s) => {
                if let Some(f) = &s.init {
                    note(&format!("{ikey}.init"), f);
                }
                for (m, f) in &s.methods {
                    note(&format!("{ikey}.{m}"), f);
                }
                for (m, f) in &s.assoc_fns {
                    note(&format!("{ikey}.{m}"), f);
                }
            }
            TypedInstantiation::Enum(_) => {}
        }
    }
    if let Some(key) = bad {
        return Err(SemaError::nowhere(
            "type",
            format!(
                "`wake` in `{key}` is outside an ISR and outside a `@task` — 03-hardware.md §6: \
                 `wake` is an ISR effect (and a bottom half may re-wake itself); ordinary code \
                 cannot"
            ),
        ));
    }
    Ok(())
}

fn fn_contains_wake(f: &TypedFn) -> bool {
    struct WakeVisitor {
        found: bool,
    }
    impl Visitor for WakeVisitor {
        fn pre_expr(&mut self, e: &TypedExpr) {
            if let TypedExprKind::Intrinsic { key, .. } = &e.kind {
                if key == "wake" {
                    self.found = true;
                }
            }
        }
    }
    let mut v = WakeVisitor { found: false };
    walk::walk_stmts(&f.body, &mut v);
    v.found
}

pub fn check_bottom_half(program: &TypedProgram) -> Result<(), SemaError> {
    let note = |key: &str, f: &TypedFn| -> Result<(), SemaError> {
        if !f.is_task {
            return Ok(());
        }
        if f.is_async {
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`@task` `{key}` is `async` — 03-hardware.md §6/§7: the bottom half never \
                     stays active while waiting (submission turns are not re-entered)"
                ),
            ));
        }
        if let Some(reason) = bottom_half_forbidden_of(f) {
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`@task` `{key}` {reason} — 03-hardware.md §6/§7: the bottom half drains a \
                     level signal and re-wakes if work remains; it does not await"
                ),
            ));
        }
        Ok(())
    };
    for (sname, s) in &program.structs {
        for (m, f) in &s.methods {
            note(&format!("{sname}.{m}"), f)?;
        }
    }
    for (ikey, inst) in &program.instantiations {
        if let TypedInstantiation::Struct(s) = inst {
            for (m, f) in &s.methods {
                note(&format!("{ikey}.{m}"), f)?;
            }
        }
    }
    Ok(())
}

fn bottom_half_forbidden_of(f: &TypedFn) -> Option<String> {
    let mut scan = BodyScan::default();
    scan_bottom_half_stmts(&f.body, &mut scan);
    scan.illegal.map(|r| format!("uses {r}"))
}

#[cfg(test)]
fn type_mentions_receipt(ty: &crate::sema::types::Type) -> bool {
    use crate::sema::types::Type;
    match ty {
        Type::Named(n, targs) => {
            n == "Receipt"
                || targs.iter().any(|a| match a {
                    crate::sema::types::TypeArg::Type(t) => type_mentions_receipt(t),
                    _ => false,
                })
        }
        Type::Option(inner) | Type::Static(inner) | Type::Own(_, inner) => {
            type_mentions_receipt(inner)
        }
        Type::Array(elem, _) => type_mentions_receipt(elem),
        Type::Tuple(elems) => elems.iter().any(type_mentions_receipt),
        Type::Result(ok, err) => type_mentions_receipt(ok) || type_mentions_receipt(err),
        _ => false,
    }
}

fn collect_isr_roots(program: &TypedProgram) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    let mut note = |f: &TypedFn| {
        let mut scan = BodyScan::default();
        scan_isr_bind_stmts(&f.body, &mut scan, &mut roots);
    };
    for f in program.fns.values() {
        note(f);
    }
    for s in program.structs.values() {
        if let Some(f) = &s.init {
            note(f);
        }
        for f in s.methods.values() {
            note(f);
        }
        for f in s.assoc_fns.values() {
            note(f);
        }
    }
    for e in program.enums.values() {
        for f in e.methods.values() {
            note(f);
        }
        for f in e.assoc_fns.values() {
            note(f);
        }
    }
    for inst in program.instantiations.values() {
        match inst {
            TypedInstantiation::Fn(f) => note(f),
            TypedInstantiation::Struct(s) => {
                if let Some(f) = &s.init {
                    note(f);
                }
                for f in s.methods.values() {
                    note(f);
                }
                for f in s.assoc_fns.values() {
                    note(f);
                }
            }
            TypedInstantiation::Enum(_) => {}
        }
    }
    roots
}

fn isr_path_from_root(
    roots: &BTreeSet<String>,
    nodes: &BTreeMap<String, NodeInfo>,
    target: &str,
) -> Option<Vec<String>> {
    if roots.contains(target) {
        return Some(vec![target.to_string()]);
    }
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: Vec<String> = roots.iter().cloned().collect();
    let mut seen: BTreeSet<String> = roots.clone();
    while let Some(cur) = queue.pop() {
        let Some(info) = nodes.get(&cur) else {
            continue;
        };
        for callee in &info.callees {
            if !seen.insert(callee.clone()) {
                continue;
            }
            parent.insert(callee.clone(), cur.clone());
            if callee == target {
                let mut path = vec![target.to_string()];
                let mut walk = target.to_string();
                while let Some(p) = parent.get(&walk) {
                    path.push(p.clone());
                    walk = p.clone();
                }
                path.reverse();
                return Some(path);
            }
            queue.push(callee.clone());
        }
    }
    None
}

fn isr_forbidden_of(program: &TypedProgram, key: &str) -> Option<String> {
    let f = lookup_typed_fn(program, key)?;
    for p in &f.params {
        if let Some(reason) = dma_touch_reason(&p.ty) {
            return Some(format!(
                "touches device DMA state via parameter `{}: {reason}`",
                p.name
            ));
        }
        if is_float_type(&p.ty) {
            return Some(format!(
                "uses floating point via parameter `{}: {}`",
                p.name,
                crate::sema::types::render_type(&p.ty)
            ));
        }
    }
    let mut scan = BodyScan::default();
    scan_isr_forbidden_stmts(&f.body, &mut scan);
    for p in &f.params {
        if let Some(def) = &p.default {
            scan_isr_forbidden_expr(def, &mut scan);
        }
    }
    scan.illegal.map(|r| format!("uses {r}"))
}

fn lookup_typed_fn<'a>(program: &'a TypedProgram, key: &str) -> Option<&'a TypedFn> {
    if let Some(f) = program.fns.get(key) {
        return Some(f);
    }
    if let Some((sname, member)) = key.split_once('.') {
        if let Some(s) = program.structs.get(sname) {
            if let Some(f) = s.methods.get(member) {
                return Some(f);
            }
            if let Some(f) = s.assoc_fns.get(member) {
                return Some(f);
            }
            if member == "init" {
                return s.init.as_ref();
            }
        }
    }
    if let Some(TypedInstantiation::Fn(f)) = program.instantiations.get(key) {
        return Some(f);
    }
    if let Some((ikey, member)) = key.rsplit_once('.') {
        if let Some(TypedInstantiation::Struct(s)) = program.instantiations.get(ikey) {
            if let Some(f) = s.methods.get(member) {
                return Some(f);
            }
            if let Some(f) = s.assoc_fns.get(member) {
                return Some(f);
            }
            if member == "init" {
                return s.init.as_ref();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema;
    use crate::sema::typed::TypedForIter;
    use crate::syntax::{lexer, parser};

    fn typed_program(src: &str) -> TypedProgram {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check_typed(&module, "test.wr").expect("test source must check")
    }

    #[test]
    fn plain_fn_is_legal() {
        let program = typed_program(
            "module examples.legal_plain

pub fn double(x: u64) -> u64:
    return x * 2
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("double"), Verdict::Legal);
    }

    #[test]
    fn transitive_chain_is_legal() {
        let program = typed_program(
            "module examples.legal_chain

fn c(x: u64) -> u64:
    return x + 1

fn b(x: u64) -> u64:
    return c(x) * 2

pub fn a(x: u64) -> u64:
    return b(x)
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("a"), Verdict::Legal);
        assert_eq!(legality.verdict("b"), Verdict::Legal);
        assert_eq!(legality.verdict("c"), Verdict::Legal);
    }

    #[test]
    fn recursion_is_legal() {
        let program = typed_program(
            "module examples.legal_recursion

pub fn countdown(n: u64) -> u64:
    if n == 0:
        return 0
    return countdown(n - 1)
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("countdown"), Verdict::Legal);
    }

    #[test]
    fn mutual_recursion_is_legal() {
        let program = typed_program(
            "module examples.legal_mutual

fn is_even(n: u64) -> bool:
    if n == 0:
        return true
    return is_odd(n - 1)

fn is_odd(n: u64) -> bool:
    if n == 0:
        return false
    return is_even(n - 1)
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("is_even"), Verdict::Legal);
        assert_eq!(legality.verdict("is_odd"), Verdict::Legal);
    }

    #[test]
    fn method_calls_are_classified_by_struct_dot_member_key() {
        let program = typed_program(
            "module examples.legal_methods

pub struct Counter:
    value: u64 = 0

    pub fn get(read self) -> u64:
        return self.value

    pub fn doubled(read self) -> u64:
        return self.get() * 2
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("Counter.get"), Verdict::Legal);
        assert_eq!(legality.verdict("Counter.doubled"), Verdict::Legal);
    }

    #[test]
    fn generic_fn_instantiation_is_classified_independently() {
        let program = typed_program(
            "module examples.legal_generic_fn

pub struct Sample:
    id: u64

    pub fn hash(read self) -> u64:
        return self.id

pub fn hash_pair[T](a: T, b: T) -> u64:
    return a.hash() ^ b.hash()

pub fn use_hash_pair(a: Sample, b: Sample) -> u64:
    return hash_pair[Sample](a, b)
",
        );
        assert!(!program.fns.contains_key("hash_pair"));
        let legality = classify(&program);
        assert_eq!(
            legality.verdict("fn:hash_pair[Sample]"),
            Verdict::Legal,
            "instantiation key spelling must match generics::canonical_key exactly"
        );
        assert_eq!(legality.verdict("use_hash_pair"), Verdict::Legal);
    }

    #[test]
    fn generic_struct_instantiation_method_is_classified_independently() {
        let program = typed_program(
            "module examples.legal_generic_struct

pub struct Slot[T, const N: usize]:
    items: [Option[T]; N]

    fn first(read self) -> Option[T]:
        return self.items[0]

pub fn use_slot() -> Option[u64]:
    slots = Slot[u64, 2](items=[None, None])
    return slots.first()
",
        );
        let legality = classify(&program);
        assert_eq!(
            legality.verdict("struct:Slot[u64, 2].first"),
            Verdict::Legal,
            "instantiated-struct method key spelling must match CalleeKey::MethodInstance"
        );
        assert_eq!(legality.verdict("use_slot"), Verdict::Legal);
    }

    #[test]
    fn closure_body_folds_into_enclosing_fn_closure_set() {
        let program = typed_program(
            "module examples.legal_closure

fn helper() -> u64:
    return 7

fn run(body: fn() -> u64) -> u64:
    return body()

pub fn use_closure() -> u64:
    return run(||: helper())
",
        );
        let f = program
            .fns
            .get("use_closure")
            .expect("use_closure must be in the typed program");
        let mut scan = BodyScan::default();
        scan_stmts(&f.body, &mut scan);
        assert!(
            scan.callees.contains("helper"),
            "use_closure's own scan must absorb its closure argument's own call to helper(), \
             got callees={:?}",
            scan.callees
        );
        assert!(scan.callees.contains("run"));

        let legality = classify(&program);
        assert_eq!(legality.verdict("use_closure"), Verdict::Legal);
        assert_eq!(legality.verdict("helper"), Verdict::Legal);
    }

    #[test]
    fn require_legal_accepts_a_legal_key() {
        let program = typed_program(
            "module examples.legal_require

pub fn double(x: u64) -> u64:
    return x * 2
",
        );
        let legality = classify(&program);
        let result = require_legal(&legality, "double", "comptime assert", Span::default());
        assert!(result.is_ok());
    }

    #[test]
    fn require_legal_reports_illegal_verdicts_with_a_call_path_diagnostic() {
        let mut verdicts = BTreeMap::new();
        verdicts.insert(
            "a".to_string(),
            Verdict::Illegal {
                path: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                reason: "await expression".to_string(),
            },
        );
        let legality = Legality { verdicts };
        let err = require_legal(
            &legality,
            "a",
            "comptime assert",
            Span {
                line: 5,
                col: 3,
                ..Default::default()
            },
        )
        .expect_err("an Illegal verdict must produce a diagnostic");
        assert_eq!(err.category, "comptime");
        assert_eq!(
            err.message,
            "`comptime assert` requires a comptime-legal closure, but `a` is not comptime-callable"
        );
        assert_eq!(err.line, 5);
        assert_eq!(err.col, 3);
        assert!(!err.omit_location);
        assert_eq!(
            err.extra_lines,
            vec![
                "  `a` calls `b`".to_string(),
                "  `b` calls `c`".to_string(),
                "  `c` uses await expression".to_string(),
            ]
        );
    }

    #[test]
    fn unknown_key_defaults_to_legal() {
        let legality = Legality::default();
        assert_eq!(legality.verdict("nonexistent"), Verdict::Legal);
        assert!(
            require_legal(&legality, "nonexistent", "comptime assert", Span::default()).is_ok()
        );
    }

    fn provenance(src: &str) -> Result<(), String> {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check(&module, "test.wr").map_err(|e| e.message)
    }

    const DRIVER_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
         @driver\n\
         pub struct D:\n\
         \x20   n: u64\n\n";

    #[test]
    fn authority_reaches_transitively_through_the_callee_graph() {
        let src = format!(
            "{DRIVER_PRELUDE}\
             \x20   fn service(read self, read m: Mmio[Regs]) -> u64:\n\
             \x20       return hop_one(m)\n\n\
             fn hop_one(read m: Mmio[Regs]) -> u64:\n\
             \x20   return hop_two(m)\n\n\
             fn hop_two(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 1\n"
        );
        assert_eq!(provenance(&src), Ok(()));
    }

    #[test]
    fn a_severed_tail_is_rejected_and_named() {
        let src = format!(
            "{DRIVER_PRELUDE}\
             \x20   fn service(read self, read m: Mmio[Regs]) -> u64:\n\
             \x20       return hop_one(m)\n\n\
             fn hop_one(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 2\n\n\
             fn hop_two(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 1\n"
        );
        let err = provenance(&src).expect_err("the severed tail must be rejected");
        assert!(err.starts_with("`hop_two` touches hardware state"), "{err}");
    }

    #[test]
    fn a_program_with_no_capability_is_untouched() {
        assert_eq!(
            provenance(
                "module t\n\nfn helper() -> u64:\n    return 1\n\n\
                 pub fn use_it() -> u64:\n    return helper()\n"
            ),
            Ok(())
        );
    }

    #[test]
    fn an_instantiation_has_no_span_and_gets_a_location_free_diagnostic() {
        let src = "module t\n\n\
             @layout(mmio, endian=little)\n\
             struct Regs:\n\
             \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
             struct Wrap:\n\
             \x20   m: Mmio[Regs]\n\
             \x20   n: u64\n\n\
             fn hold[T](read m: T) -> u64:\n\
             \x20   return 0\n\n\
             fn zz_caller(read w: Wrap) -> u64:\n\
             \x20   return hold[Wrap](w)\n";
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let err = sema::check(&module, "test.wr").expect_err("the instantiation touches");
        assert!(
            err.message
                .starts_with("`fn:hold[Wrap]` touches hardware state"),
            "{}",
            err.message
        );
        assert!(
            err.omit_location,
            "an instantiation has no location to cite"
        );
    }

    #[test]
    fn an_actor_is_not_an_authority_root() {
        let src = "module t\n\n\
             @layout(mmio, endian=little)\n\
             struct Regs:\n\
             \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
             @actor\n\
             pub struct A:\n\
             \x20   n: u64\n\n\
             \x20   init(mut self):\n\
             \x20       self.n = 0\n\n\
             \x20   fn call_it(read self) -> u64:\n\
             \x20       return 0\n\n\
             fn holds(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 1\n";
        let err = provenance(src).expect_err("an actor confers no authority");
        assert!(err.starts_with("`holds` touches hardware state"), "{err}");
    }

    const MMIO_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\n";

    #[test]
    fn an_mmio_access_is_a_hardware_touch_named_by_its_operation() {
        let read = format!(
            "{MMIO_PRELUDE}struct Bundle:\n\
             \x20   regs: Mmio[Regs]\n\n\
             \x20   fn peek(read self) -> u32:\n\
             \x20       return self.regs.status.read()\n\n\
             @driver\npub struct D:\n    n: u64\n"
        );
        let err = provenance(&read).expect_err("no driver reaches `Bundle.peek`");
        assert!(
            err.starts_with("`Bundle.peek` touches hardware state (an MMIO register read)"),
            "{err}"
        );

        let write = format!(
            "{MMIO_PRELUDE}@layout(mmio, endian=little)\n\
             struct Ack:\n\
             \x20   @offset(0x010) ack: WriteOnly[u32]\n\n\
             struct Bundle:\n\
             \x20   regs: Mmio[Ack]\n\n\
             \x20   fn poke(read self):\n\
             \x20       self.regs.ack.write(1)\n\n\
             @driver\npub struct D:\n    n: u64\n"
        );
        let err = provenance(&write).expect_err("no driver reaches `Bundle.poke`");
        assert!(
            err.starts_with("`Bundle.poke` touches hardware state (an MMIO register write)"),
            "{err}"
        );
    }

    #[test]
    fn an_mmio_access_is_comptime_illegal() {
        let program = typed_program(&format!(
            "{MMIO_PRELUDE}@driver\npub struct D:\n\
             \x20   regs: Mmio[Regs]\n\n\
             \x20   fn peek(read self) -> u32:\n\
             \x20       return self.regs.status.read()\n\n\
             \x20   fn peek_twice(read self) -> u32:\n\
             \x20       return self.peek() + self.peek()\n"
        ));
        let legality = classify(&program);
        assert_eq!(
            legality.verdict("D.peek"),
            Verdict::Illegal {
                path: vec!["D.peek".to_string()],
                reason: "a volatile MMIO register access".to_string(),
            }
        );
        assert!(matches!(
            legality.verdict("D.peek_twice"),
            Verdict::Illegal { .. }
        ));
    }

    #[test]
    fn isr_forbidden_reason_names_every_doc_effect() {
        use crate::sema::types::Type;
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::While {
                cond: TypedExpr {
                    span: Span::default(),
                    ty: Type::Bool,
                    kind: TypedExprKind::Bool(true),
                },
                body: vec![],
                budget: None,
            }),
            Some("a loop (drain unbounded work — loops belong in the bottom half)")
        );
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::For {
                name: "i".into(),
                elem_ty: Type::U64,
                take_binding: false,
                iter: TypedForIter::Range(
                    TypedExpr {
                        span: Span::default(),
                        ty: Type::U64,
                        kind: TypedExprKind::Int("0".into()),
                    },
                    TypedExpr {
                        span: Span::default(),
                        ty: Type::U64,
                        kind: TypedExprKind::Int("1".into()),
                    },
                    false,
                ),
                body: vec![],
                budget: None,
            }),
            Some("a loop (drain unbounded work — loops belong in the bottom half)")
        );
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::BareSend {
                span: Span::default(),
                expr: TypedExpr {
                    span: Span::default(),
                    ty: Type::Unit,
                    kind: TypedExprKind::Unit,
                },
            }),
            Some("a bare `send` (call another actor)")
        );
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::WithGroup {
                capacity: None,
                deadline: None,
                as_name: None,
                body: vec![],
            }),
            Some("a `with group` (call another actor / block)")
        );
        let await_expr = TypedExpr {
            span: Span::default(),
            ty: Type::U64,
            kind: TypedExprKind::Await(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::U64,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(expr_isr_forbidden_reason(&await_expr), Some("an `await`"));
        let send_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Send(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::Unit,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(
            expr_isr_forbidden_reason(&send_expr),
            Some("a `send` (call another actor)")
        );
        let float_expr = TypedExpr {
            span: Span::default(),
            ty: Type::F64,
            kind: TypedExprKind::Float("1.0".into()),
        };
        assert_eq!(
            expr_isr_forbidden_reason(&float_expr),
            Some("floating point")
        );
        let format_expr = TypedExpr {
            span: Span::default(),
            ty: Type::String(Box::new(crate::syntax::ast::Expr::Int(
                Span::default(),
                "10".into(),
            ))),
            kind: TypedExprKind::Call {
                callee: crate::sema::typed::CalleeKey::Method("u32".into(), "format".into()),
                receiver: Some(Box::new(TypedExpr {
                    span: Span::default(),
                    ty: Type::U32,
                    kind: TypedExprKind::Int("1".into()),
                })),
                args: vec![],
            },
        };
        assert_eq!(
            expr_isr_forbidden_reason(&format_expr),
            Some("formatting (f-string / Format)")
        );
        let wake_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Intrinsic {
                key: "wake".into(),
                receiver: None,
                type_arg: None,
                const_arg: None,
                args: vec![],
            },
        };
        assert_eq!(expr_isr_forbidden_reason(&wake_expr), None);
        let entropy_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Bytes(Some(Box::new(crate::syntax::ast::Expr::Int(
                Span::default(),
                "8".into(),
            )))),
            kind: TypedExprKind::Intrinsic {
                key: "entropy".into(),
                receiver: None,
                type_arg: None,
                const_arg: Some(8),
                args: vec![],
            },
        };
        assert_eq!(
            expr_isr_forbidden_reason(&entropy_expr),
            Some("a runtime entropy fill (`entropy[N]()` — not in the ISR effect set)")
        );
        assert_eq!(
            effect_reason(EffectClass::Entropy, EffectContext::Comptime),
            Some("`entropy[N]()` (a runtime-only entropy fill)")
        );
        assert_eq!(
            effect_reason(EffectClass::Entropy, EffectContext::BottomHalf),
            None
        );
        assert!(type_mentions_receipt(&Type::Named(
            "Receipt".into(),
            vec![crate::sema::types::TypeArg::Type(Type::U32)]
        )));
    }

    #[test]
    fn bottom_half_forbidden_reason_names_await_send_not_receipt() {
        use crate::sema::types::Type;
        let await_expr = TypedExpr {
            span: Span::default(),
            ty: Type::U64,
            kind: TypedExprKind::Await(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::U64,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(
            bottom_half_expr_forbidden_reason(&await_expr),
            Some("an `await` (stays active while waiting)")
        );
        let send_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Send(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::Unit,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(
            bottom_half_expr_forbidden_reason(&send_expr),
            Some("a `send` (call another actor)")
        );
        let receipt_ty = Type::Named(
            "Receipt".into(),
            vec![crate::sema::types::TypeArg::Type(Type::U32)],
        );
        let receipt_expr = TypedExpr {
            span: Span::default(),
            ty: receipt_ty,
            kind: TypedExprKind::Unit,
        };
        assert_eq!(bottom_half_expr_forbidden_reason(&receipt_expr), None);
    }
}
