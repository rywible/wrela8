//! Sealed-transport checkers (plans/M7.md item H1, 03-hardware.md §9).
//! Extracted from `bodies.rs` along the artifact boundary.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::bodies::{
    FnCtx, ModuleCtx, check_expr, parse_int_literal, type_error, unwrap_own,
};
use crate::sema::typed::{CalleeKey, TypedCallArg, TypedExpr, TypedExprKind};
use crate::sema::types::{self, DeclMember, DeclParam, Type, TypeArg};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{self, AccessMode, Arg, BinOp, Expr, Member, NamedType, Span, Stmt};

// --- plans/M7.md item H1: 03-hardware.md §9's sealed transport ------------
//
// The bring-up chain, and the two of its operations this item makes real.
//
// ## The chain, and what a state *is*
//
// `Reset -> Acknowledged -> DriverClaimed -> FeaturesNegotiated ->
// FeaturesAccepted -> QueuesConfigured -> Running`, one builtin type per
// state (`eval::image_checks::PROTOCOL_STATE_TYPES`), each carrying the
// device type — `RunningDevice[VirtioBlock]` is the docs' own spelling and
// the other six follow it. Every one is a resource, which is not a
// decoration: §9's "each fallible transition **consumes** its input state"
// *is* the resource rule, and the only reason a transition can consume one
// is that it is never implicitly copied.
//
// ## `claim`, and why it emits nothing on this target
//
// `VirtioBlock.claim(cap=take cap)` consumes the `DeviceCap[D]` and yields
// `DriverClaimedDevice[D]` — the docs' own comment on the line is "reset +
// acknowledge", i.e. the three status writes a real virtio transport needs
// to walk `Reset -> Acknowledged -> DriverClaimed`. **This machine has no
// status register to write.** 06-machine.md §3: "no discovery ... the VMM
// preconfigures every device, queue, and shared-memory window the report
// declares — device topology is a *build output*, not a probed fact", and
// "cold boot is a design property: there is nothing to negotiate". The VMM
// has no `MagicValue`/`DeviceID`/`Status` register file at all
// (`wrela-vmm::devices`' module doc). So on machine v1 `claim` is a pure
// authority transition: it carries the device's base address forward and
// emits no access. That is a target fact, recorded, not an omission — and
// it is exactly why the *first* MMIO this compiler ever emits is the
// driver's own ISR partition rather than a status write.
//
// ## `map_partition`, and how it feeds item C's rule instead of dodging it
//
// `claimed.map_partition(VirtioIrqMmio)` yields `Mmio[VirtioIrqMmio]`.
// 03 §2: "a driver **or sealed protocol** partitions its claim into
// declared, non-overlapping layouts ... minting a layout consumes those
// byte ranges from the claim". Item C built the *rule* over a driver's
// declared `Mmio[L]` **fields**; this is the *operation*, so the operation
// is constrained to that same set: `map_partition(L)` is legal only inside
// a `@driver` that declares `Mmio[L]` in a field. A partition the no-alias
// rule never saw therefore cannot exist, and the `devregs` window that
// backs the claim is sized from the identical set
// (`layout::device_register_windows`).
//
// ## What is deliberately not here
//
// `negotiate`/`start`/`read_capacity_sectors`/`take_irq`/`VirtQueue.configure`
// are each a named rejection carrying the state they would consume, the
// state they would produce, and what is actually missing. `negotiate` in
// particular is *not* merely unimplemented: on this machine the accepted
// feature set is decided before the guest runs (item F's VMM-side
// `negotiate`, against the image's declared `required_features`), and
// nothing carries that result into the guest — there is no declared window
// for it and no plan item has claimed one. Failing closed says so.

/// The device type an `Mmio`/state type argument names, if it names one.
pub(crate) fn device_type_arg(targs: &[types::TypeArg]) -> Option<&str> {
    match targs.first() {
        Some(types::TypeArg::Type(Type::Named(d, _))) => Some(d.as_str()),
        _ => None,
    }
}

/// Intrinsic args are `(label, TypedExpr)` with no mode slot. AST
/// `take`/`mut` markers live on `Arg.mode`; wrap `take` so typed
/// flow/access see `TypedExprKind::Take` and mark the place Moved
/// (protocol-resource consumption). `mut` stays a bare place — overlap
/// tracking for Intrinsic mut args is weaker than `TypedCallArg`, but
/// no Intrinsic today needs that for correctness.
fn with_arg_mode(mode: AccessMode, value: TypedExpr, span: Span) -> TypedExpr {
    match mode {
        AccessMode::Take => take_place(value, span),
        AccessMode::Read | AccessMode::Mut => value,
    }
}

/// Wrap a place the sealed transport consumes (03-hardware.md §9:
/// each fallible transition consumes its input state).
fn take_place(value: TypedExpr, span: Span) -> TypedExpr {
    TypedExpr {
        span,
        ty: value.ty.clone(),
        kind: TypedExprKind::Take(Box::new(value)),
    }
}

/// `<Device>.claim(cap=take cap)` (03-hardware.md §9).
pub(crate) fn check_device_claim(
    device: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{device}.claim(cap=take cap)` takes exactly one argument, the `DeviceCap[{device}]` \
                 the image minted; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if arg.label.as_deref() != Some("cap") {
        return Err(type_error(
            format!(
                "`{device}.claim`'s own argument is labelled `cap=` (03-hardware.md §9's own \
                 spelling: `{device}.claim(cap=take cap)`)"
            ),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            format!(
                "`{device}.claim` consumes the capability: write `cap=take ...` \
                 (03-hardware.md §9 — each transition consumes its input)"
            ),
            arg.span,
        ));
    }
    let expected = Type::Named(
        "DeviceCap".to_string(),
        vec![types::TypeArg::Type(Type::Named(
            device.to_string(),
            vec![],
        ))],
    );
    let cap = with_arg_mode(
        arg.mode,
        check_expr(&arg.value, Some(&expected), fctx, mctx)?,
        arg.span,
    );
    let cap_ty = unwrap_own(cap.ty.clone());
    let Type::Named(cap_name, cap_targs) = &cap_ty else {
        return Err(type_error(
            format!(
                "`{device}.claim`'s own `cap=` is a `DeviceCap[{device}]`; found `{}`",
                types::render_type(&cap.ty)
            ),
            arg.span,
        ));
    };
    if cap_name != "DeviceCap" || device_type_arg(cap_targs) != Some(device) {
        return Err(type_error(
            format!(
                "`{device}.claim`'s own `cap=` is a `DeviceCap[{device}]` — authority over *this* \
                 device (03-hardware.md §1); found `{}`",
                types::render_type(&cap.ty)
            ),
            arg.span,
        ));
    }
    let _ = fspan;
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named(
            "DriverClaimedDevice".to_string(),
            vec![types::TypeArg::Type(Type::Named(
                device.to_string(),
                vec![],
            ))],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "Device.claim".to_string(),
            receiver: None,
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            const_arg: None,
            args: vec![("cap".to_string(), cap)],
        },
    })
}

/// A method call on one of 03 §9's bring-up states.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_device_state_call(
    state_expr: TypedExpr,
    state: &str,
    targs: &[types::TypeArg],
    method: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let rendered = types::render_type(&state_expr.ty);
    let device = device_type_arg(targs).unwrap_or("?").to_string();
    match method {
        "map_partition" => {
            check_map_partition(state_expr, &rendered, args, fspan, call_span, fctx, mctx)
        }
        // plans/M7.md item E1, decision 14: `negotiate` is a **build-time**
        // fact. Both sides (the image's `required_features`, and
        // `virtqueue::DEVICE_FEATURES`) are build outputs; an unofferable
        // required feature fails the *build*, and the guest's call is a
        // pure authority transition that always yields
        // `Ok(FeaturesAcceptedDevice[D])`. The call-site `required=`/
        // `optional=` arrays are shape-checked here; the bits themselves
        // are checked against the model when the image seals
        // (`check_blk_device_features`).
        "negotiate" => check_device_negotiate(
            state_expr, state, &device, &rendered, args, fspan, call_span, fctx, mctx,
        ),
        "start" => check_device_start(
            state_expr, state, &device, &rendered, args, fspan, call_span,
        ),
        "reset" => check_device_reset(
            state_expr, state, &device, &rendered, args, fspan, call_span, fctx, mctx,
        ),
        "read_capacity_sectors" => check_device_read_capacity(
            state_expr, state, &device, &rendered, args, fspan, call_span,
        ),
        "take_irq" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!(
                        "`{rendered}.take_irq()` takes no arguments; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            }
            let _ = fspan;
            Ok(TypedExpr {
                span: call_span,
                ty: Type::Named("IrqCap".to_string(), vec![types::TypeArg::Type(Type::U32)]),
                kind: TypedExprKind::Intrinsic {
                    key: "Device.take_irq".to_string(),
                    receiver: Some(Box::new(state_expr)),
                    type_arg: None,
                    const_arg: None,
                    args: Vec::new(),
                },
            })
        }
        other => Err(type_error(
            format!(
                "`{rendered}` has no operation `{other}`; 03-hardware.md §9's bring-up chain \
                 gives a claimed device `map_partition`, `negotiate`, `read_capacity_sectors`, \
                 `take_irq` and `start`; `reset` consumes a `RunningDevice` (plans/M7.md item H2b)"
            ),
            fspan,
        )),
    }
}

pub(crate) fn boot_error_ty() -> Type {
    Type::Named("BootError".to_string(), vec![])
}

pub(crate) fn device_state_ty(state: &str, device: &str) -> Type {
    Type::Named(
        state.to_string(),
        vec![types::TypeArg::Type(Type::Named(
            device.to_string(),
            vec![],
        ))],
    )
}

/// `claimed.negotiate(required=..., optional=...)` — DriverClaimed ->
/// FeaturesAccepted (Result). plans/M7.md decision 14.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_device_negotiate(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if state != "DriverClaimedDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.negotiate(...)` consumes a `DriverClaimedDevice[{device}]` \
                 (03-hardware.md §9: `DriverClaimed -> FeaturesAccepted`); found `{rendered}`"
            ),
            fspan,
        ));
    }
    if args.len() != 2 {
        return Err(type_error(
            format!(
                "`{rendered}.negotiate(required=..., optional=...)` takes exactly two labelled \
                 arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut required = None;
    let mut optional = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("required") => {
                if required.is_some() {
                    return Err(type_error(
                        "`negotiate`'s `required=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`negotiate`'s `required=` is a feature list, not a moved value: \
                             drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                required = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some("optional") => {
                if optional.is_some() {
                    return Err(type_error(
                        "`negotiate`'s `optional=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`negotiate`'s `optional=` is a feature list, not a moved value: \
                             drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                optional = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`negotiate`'s own arguments are labelled `required=` and `optional=`; \
                         `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`negotiate(required=..., optional=...)` requires labelled arguments \
                     (03-hardware.md §9 / docs/language/examples/virtio-storage.wr)"
                        .to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (Some(required), Some(optional)) = (required, optional) else {
        return Err(type_error(
            format!(
                "`{rendered}.negotiate` needs both `required=` and `optional=` \
                 (03-hardware.md §9)"
            ),
            call_span,
        ));
    };
    // Feature lists are arrays (or empty-looking literals). Their element
    // type is a user enum of feature names; the *bits* are checked at
    // image seal, not here — this is the shape half.
    for (label, expr) in [("required", &required), ("optional", &optional)] {
        match &expr.ty {
            Type::Array(_, _) => {}
            other => {
                return Err(type_error(
                    format!(
                        "`negotiate`'s `{label}=` is a feature list (`[...]`); found `{}`",
                        types::render_type(other)
                    ),
                    call_span,
                ));
            }
        }
    }
    let _ = fspan;
    let accepted = device_state_ty("FeaturesAcceptedDevice", device);
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Result(Box::new(accepted), Box::new(boot_error_ty())),
        kind: TypedExprKind::Intrinsic {
            key: "Device.negotiate".to_string(),
            receiver: Some(Box::new(take_place(state_expr, call_span))),
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            const_arg: None,
            args: vec![
                ("required".to_string(), required),
                ("optional".to_string(), optional),
            ],
        },
    })
}

/// `negotiated.start()` — QueuesConfigured -> Running (infallible on
/// this machine: the queue was already placed at configure).
pub(crate) fn check_device_start(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
) -> Result<TypedExpr, SemaError> {
    if state != "QueuesConfiguredDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.start()` consumes a `QueuesConfiguredDevice[{device}]` \
                 (03-hardware.md §9's final `-> Running` transition); found `{rendered}`. \
                 Call `VirtQueue.configure(...)` first"
            ),
            fspan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`{rendered}.start()` takes no arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    Ok(TypedExpr {
        span: call_span,
        ty: device_state_ty("RunningDevice", device),
        kind: TypedExprKind::Intrinsic {
            key: "Device.start".to_string(),
            receiver: Some(Box::new(take_place(state_expr, call_span))),
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            const_arg: None,
            args: Vec::new(),
        },
    })
}

/// `running.reset(queue=mut q)` — Running -> Running with a new epoch
/// (plans/M7.md item H2b / decision 23). Full device reset on machine v1;
/// per-queue reset is a typed rejection on `VirtQueue.reset`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_device_reset(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if state != "RunningDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.reset(...)` consumes a `RunningDevice[{device}]` \
                 (03-hardware.md §9: reset consumes `Running`, producing a new epoch); \
                 found `{rendered}`"
            ),
            fspan,
        ));
    }
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{rendered}.reset(queue=mut q)` takes exactly one labelled argument; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if arg.label.as_deref() != Some("queue") {
        return Err(type_error(
            format!(
                "`{rendered}.reset`'s own argument is labelled `queue=` (plans/M7.md item H2b: \
                 the epoch lives in the queue's control-pool bookkeeping)"
            ),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Mut {
        return Err(type_error(
            format!(
                "`{rendered}.reset` mutates the queue's live epoch in place: write `queue=mut ...`"
            ),
            arg.span,
        ));
    }
    let queue = check_expr(&arg.value, None, fctx, mctx)?;
    let queue_ty = unwrap_own(queue.ty.clone());
    let Type::Named(qname, _) = &queue_ty else {
        return Err(type_error(
            format!(
                "`{rendered}.reset`'s `queue=` is a `VirtQueue[..N]`; found `{}`",
                types::render_type(&queue.ty)
            ),
            arg.span,
        ));
    };
    if qname != "VirtQueue" {
        return Err(type_error(
            format!(
                "`{rendered}.reset`'s `queue=` is a `VirtQueue[..N]`; found `{}`",
                types::render_type(&queue.ty)
            ),
            arg.span,
        ));
    }
    let _ = fspan;
    Ok(TypedExpr {
        span: call_span,
        ty: device_state_ty("RunningDevice", device),
        kind: TypedExprKind::Intrinsic {
            key: "Device.reset".to_string(),
            receiver: Some(Box::new(take_place(state_expr, call_span))),
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            const_arg: None,
            args: vec![("queue".to_string(), queue)],
        },
    })
}

/// `negotiated.read_capacity_sectors()` — capacity is an image-declared,
/// report-carried fact (`BlkDevice capacity_sectors=`). The guest call
/// lowers to that build constant (decision recorded with decision 14);
/// there is no config register to read on this machine.
pub(crate) fn check_device_read_capacity(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
) -> Result<TypedExpr, SemaError> {
    if state != "FeaturesAcceptedDevice" && state != "QueuesConfiguredDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.read_capacity_sectors()` is a virtio-blk config read on a \
                 features-accepted (or queues-configured) device; found `{rendered}`"
            ),
            fspan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`{rendered}.read_capacity_sectors()` takes no arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let _ = device;
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Result(Box::new(Type::U64), Box::new(boot_error_ty())),
        kind: TypedExprKind::Intrinsic {
            key: "Device.read_capacity_sectors".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
    })
}

/// `<state>.map_partition(L)` (03-hardware.md §2/§9).
pub(crate) fn check_map_partition(
    state_expr: TypedExpr,
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
                "`{rendered}.map_partition(L)` takes exactly one argument, the `@layout(mmio)` \
                 type to map; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!("`map_partition(L)`'s layout is positional; `{label}=` names no parameter"),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`map_partition(L)`'s argument is a *type*, not a value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let Expr::Name(_, layout_name) = &arg.value else {
        return Err(type_error(
            "`map_partition(L)`'s argument names an `@layout(mmio)` type (03-hardware.md §2), \
             not a value"
                .to_string(),
            arg.span,
        ));
    };
    match mctx.layouts.get(layout_name.as_str()) {
        Some(l) if l.kind == types::LayoutKind::Mmio => {}
        _ => {
            return Err(type_error(
                format!(
                    "`map_partition({layout_name})` requires `{layout_name}` to be an \
                     `@layout(mmio)` struct (03-hardware.md §2: a typed register layout)"
                ),
                arg.span,
            ));
        }
    }
    // 03 §2's partition rule, wired to item C's own check rather than
    // restated: the layouts a `@driver` mints are exactly the ones its
    // declared `Mmio[L]` fields name, `check_mmio_claims` proves *those*
    // pairwise disjoint, and `layout::device_register_windows` sizes the
    // claim's window from the same set. A `map_partition` of anything else
    // would be a live layout no rule ever ranged over.
    let Some(Type::Named(owner, _)) = fctx.lookup_local("self").map(unwrap_own) else {
        return Err(type_error(
            format!(
                "`{rendered}.map_partition({layout_name})` partitions a `@driver`'s own claim \
                 (03-hardware.md §2), so it is only callable from inside one"
            ),
            call_span,
        ));
    };
    let structs: std::collections::BTreeMap<String, &types::DeclStruct> = mctx
        .structs
        .iter()
        .map(|(n, s)| (n.clone(), &s.decl))
        .collect();
    // The nesting table item I's sweep made this walk need: a layout
    // reached through a wrapper struct *or* an enum variant payload, which
    // is why enums are here beside structs (`types::components_by_name`'s
    // own content, built from this pass's own already-declared tables).
    let components: std::collections::BTreeMap<String, &[(Type, Span)]> = mctx
        .structs
        .iter()
        .map(|(n, s)| (n.clone(), s.decl.component_types.as_slice()))
        .chain(
            mctx.enums
                .iter()
                .map(|(n, e)| (n.clone(), e.component_types.as_slice())),
        )
        .collect();
    let Some(mints) = types::mmio_mints_of(&owner, &structs, &components) else {
        return Err(type_error(
            format!(
                "`map_partition({layout_name})` partitions a `@driver`'s own claim, and \
                 `{owner}` is not a `@driver` (03-hardware.md §2)"
            ),
            call_span,
        ));
    };
    if !mints.iter().any(|m| m == layout_name) {
        return Err(type_error(
            format!(
                "`@driver` `{owner}` maps `{layout_name}`, but declares no field of type \
                 `Mmio[{layout_name}]`. A driver's declared `Mmio[L]` fields *are* its partition \
                 of the claim (03-hardware.md §2), and they are what the no-alias rule and the \
                 device's own register window are both derived from — a partition mapped outside \
                 that set would be a live layout no rule ever saw{}",
                if mints.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; `{owner}` declares {}",
                        mints
                            .iter()
                            .map(|m| format!("`Mmio[{m}]`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ),
            call_span,
        ));
    }
    let _ = fspan;
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named(
            "Mmio".to_string(),
            vec![types::TypeArg::Type(Type::Named(
                layout_name.clone(),
                vec![],
            ))],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "Device.map_partition".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: Some(Type::Named(layout_name.clone(), vec![])),
            const_arg: None,
            args: Vec::new(),
        },
    })
}

/// Is `key` one of item H1's sealed-transport intrinsics, including item
/// G's `take_irq`? Same three-consumer discipline as
/// `is_mmio_access_intrinsic` above.
pub fn is_device_transport_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "Device.claim"
            | "Device.map_partition"
            | "Device.negotiate"
            | "Device.start"
            | "Device.reset"
            | "Device.read_capacity_sectors"
            | "Device.take_irq"
            | "VirtQueue.configure"
    )
}

/// plans/M7.md item E2/E3/E4 / G fail-closed keys — used by lower and
/// flowwir so an unimplemented queue/IRQ op names its owner rather than
/// falling into a generic "intrinsic" rejection.
pub fn is_queue_op_deferred(key: &str) -> Option<&'static str> {
    match key {
        "VirtQueue.poll_sources" | "VirtQueue.completions_pending" => {
            Some("plans/M7.md item G (`poll_sources` / `completions_pending`)")
        }
        _ => None,
    }
}

/// Is `key` one of item E2/E3/E4's live queue operations?
pub fn is_queue_op_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "VirtQueue.reserve"
            | "VirtQueue.prepare_block"
            | "VirtQueue.publish"
            | "VirtQueue.reject"
            | "VirtQueue.drain"
            | "VirtQueue.suppress_interrupts"
            | "VirtQueue.claim"
            | "VirtQueue.recover"
            | "VirtQueue.reclaim"
    )
}

/// A method call on a `VirtQueue[..N]` value (03-hardware.md §4).
pub(crate) fn check_virtqueue_method(
    queue: TypedExpr,
    name: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match name {
        "reserve" => {
            check_virtqueue_reserve(queue, args, fspan, call_span, expected, fctx, mctx)
        }
        "prepare_block" => check_virtqueue_prepare_block(queue, args, fspan, call_span, fctx, mctx),
        "publish" => check_virtqueue_publish(queue, args, fspan, call_span, fctx, mctx),
        "reject" => check_virtqueue_reject(queue, args, fspan, call_span, fctx, mctx),
        "drain" => check_virtqueue_drain(queue, args, fspan, call_span, fctx, mctx),
        "reset" => Err(type_error(
            "`VirtQueue.reset` is per-queue reset, which requires the `RingReset` feature \
             this device model does not offer (03-hardware.md §9: \"per-queue reset (when \
             negotiated)\"; plans/M7.md item H2b / decision 23: machine v1 does full \
             `RunningDevice.reset(queue=mut ...)` only — see `golden/err-device-required-unoffered`)"
                .to_string(),
            fspan,
        )),
        "suppress_interrupts" => {
            check_virtqueue_suppress_interrupts(queue, args, fspan, call_span, fctx, mctx)
        }
        "claim" => check_virtqueue_claim(queue, args, fspan, call_span, fctx, mctx),
        "recover" => check_virtqueue_recover(queue, args, fspan, call_span, fctx, mctx),
        "reclaim" => check_virtqueue_reclaim(queue, args, fspan, call_span, fctx, mctx),
        "poll_sources" | "completions_pending" => Err(unimplemented_at(
            &format!("`VirtQueue.{name}(...)` — plans/M7.md item G (`{name}`) is"),
            call_span,
        )),
        other => Err(type_error(
            format!(
                "`VirtQueue[..N]` has no method `{other}`; 03-hardware.md §4/§5/§9 give \
                 `reserve`, `prepare_block`, `publish`, `reject`, `drain`, \
                 `suppress_interrupts`, `claim`, `recover`, and `reclaim`"
            ),
            fspan,
        )),
    }
}

/// `queue.reserve(descriptors=3)` — declared
/// `Result[QueuePermit, CapacityError]`; when `expected` is `QueuePermit`,
/// returns that type and records a permit demand for `sema::reserve_proof`
/// (plans/M13.md item M).
pub(crate) fn check_virtqueue_reserve(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let depth = virtqueue_type_depth(&queue.ty, mctx).ok_or_else(|| {
        type_error(
            "`reserve` needs a `VirtQueue[..N]` whose depth is a comptime-known \
             nonzero power of two (03-hardware.md §4)"
                .to_string(),
            call_span,
        )
    })?;
    if depth == 0 || !depth.is_power_of_two() {
        return Err(type_error(
            format!("`reserve` on `VirtQueue[..{depth}]`: depth must be a nonzero power of two"),
            call_span,
        ));
    }
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.reserve(descriptors=N)` takes exactly one labelled argument; \
                 found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("descriptors") {
        return Err(type_error(
            "`VirtQueue.reserve`'s own argument is labelled `descriptors=` \
             (03-hardware.md §4)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`reserve`'s `descriptors=` is a count, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let desc_expr = check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?;
    let desc_val = virtqueue_depth_value(&desc_expr, mctx).ok_or_else(|| {
        type_error(
            "`reserve`'s `descriptors=` must be a comptime-known integer \
             (03-hardware.md §4)"
                .to_string(),
            arg.span,
        )
    })?;
    if desc_val == 0 || desc_val > u64::from(u16::MAX) {
        return Err(type_error(
            format!("`reserve(descriptors={desc_val})` is not a usable descriptor count"),
            arg.span,
        ));
    }
    if desc_val != u64::from(crate::virtqueue::DESCRIPTORS_PER_BLK_OP) {
        return Err(type_error(
            format!(
                "`reserve(descriptors={desc_val})`: machine v1's virtio-blk operation \
                 uses exactly {} descriptors (header + data + status)",
                crate::virtqueue::DESCRIPTORS_PER_BLK_OP
            ),
            arg.span,
        ));
    }
    let _ = fspan;
    // plans/M13.md item M / decision 1: `reserve`'s declared type is
    // `Result[QueuePermit, CapacityError]`. When the use site expects
    // `QueuePermit` (threaded from `check_call_by_field` → here), return
    // that type and push the demand for `reserve_proof`. Locals bound
    // from a Result-typed `reserve` still collapse in `check_expr` when
    // later used as `QueuePermit`. Otherwise leave the Result and item L
    // refuses silent `Err` discard.
    // Encode the resolved depth as a literal Bound on `type_arg` so
    // `sema::reserve_proof` never has to re-resolve a const name.
    let permit = Type::Named("QueuePermit".to_string(), vec![]);
    let ty = if matches!(
        expected,
        Some(Type::Named(n, targs)) if n == "QueuePermit" && targs.is_empty()
    ) {
        mctx.reserve_permit_demands.borrow_mut().push(call_span);
        permit
    } else {
        Type::Result(
            Box::new(permit),
            Box::new(Type::Named("CapacityError".to_string(), vec![])),
        )
    };
    Ok(TypedExpr {
        span: call_span,
        ty,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.reserve".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: Some(Type::Named(
                "VirtQueue".to_string(),
                vec![types::TypeArg::Bound(Expr::Int(
                    call_span,
                    depth.to_string(),
                ))],
            )),
            const_arg: None,
            args: vec![("descriptors".to_string(), desc_expr)],
        },
    })
}

/// plans/M8.md item G, decision 18: the one wording for 03-hardware.md §9's
/// no-auto-retry rule, shared by the two sites that can commit the
/// violation (`prepare_block` builds the operation; `publish` issues it).
/// One message, two sites — a hoisted `prepare_block` and an inlined one
/// are the same mistake and read the same way.
pub(crate) fn no_auto_retry_message(site: &str) -> String {
    format!(
        "`{site}` re-issues an operation declared `idempotent=false` inside a \
         `CompletionOutcome.Unknown` arm — 03-hardware.md §9: \"Source must not auto-retry a \
         non-idempotent operation on `Unknown`\". The first attempt may already have taken \
         effect, so retrying it can apply the operation twice. Either establish quiescence \
         first (quarantine the device and pool, or go target-fatal — 03 §9), or, if re-running \
         this exact operation is provably harmless, declare it `idempotent=true` at its \
         `prepare_block`"
    )
}

/// `queue.prepare_block(permit=take ..., header=..., payload=take ...,
/// device_writes_payload=..., status=..., idempotent=...)` — yields a
/// `QueueOp[P, <idempotent>]`.
pub(crate) fn check_virtqueue_prepare_block(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 6 {
        return Err(type_error(
            format!(
                "`VirtQueue.prepare_block(permit=take ..., header=..., payload=take ..., \
                 device_writes_payload=..., status=..., idempotent=...)` takes exactly six \
                 labelled arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut permit = None;
    let mut header = None;
    let mut payload = None;
    let mut device_writes = None;
    let mut status = None;
    let mut idempotent: Option<bool> = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("permit") => {
                if permit.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `permit=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`prepare_block` consumes the permit: write `permit=take ...` \
                         (03-hardware.md §4)"
                            .to_string(),
                        arg.span,
                    ));
                }
                let expected = Type::Named("QueuePermit".to_string(), vec![]);
                permit = Some(with_arg_mode(
                    arg.mode,
                    check_expr(&arg.value, Some(&expected), fctx, mctx)?,
                    arg.span,
                ));
            }
            Some("header") => {
                if header.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `header=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `header=` is a `@layout(dma)` value, not a moved \
                             handle: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let h = check_expr(&arg.value, None, fctx, mctx)?;
                require_layout_dma(&h.ty, "header", arg.span, mctx)?;
                header = Some(h);
            }
            Some("payload") => {
                if payload.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `payload=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`prepare_block` consumes the transfer payload: write `payload=take ...` \
                         (03-hardware.md §3/§4)"
                            .to_string(),
                        arg.span,
                    ));
                }
                let p = check_expr(&arg.value, None, fctx, mctx)?;
                match &p.ty {
                    Type::Own(_, inner) => {
                        require_layout_dma(inner, "payload", arg.span, mctx)?;
                    }
                    other => {
                        return Err(type_error(
                            format!(
                                "`prepare_block`'s `payload=` is an `own[P] T` transfer handle \
                                 (03-hardware.md §3); found `{}`",
                                types::render_type(other)
                            ),
                            arg.span,
                        ));
                    }
                }
                payload = Some(with_arg_mode(arg.mode, p, arg.span));
            }
            Some("device_writes_payload") => {
                if device_writes.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `device_writes_payload=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `device_writes_payload=` is a bool, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                device_writes = Some(check_expr(&arg.value, Some(&Type::Bool), fctx, mctx)?);
            }
            Some("status") => {
                if status.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `status=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `status=` is a `@layout(dma)` value, not a moved \
                             handle: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let s = check_expr(&arg.value, None, fctx, mctx)?;
                require_layout_dma(&s.ty, "status", arg.span, mctx)?;
                status = Some(s);
            }
            // plans/M8.md item G, decision 18: 03-hardware.md §9's
            // no-auto-retry rule needs to know whether re-running this
            // operation is harmless, and **nothing in the compiler can
            // work that out** — a write of fixed bytes to a fixed sector
            // is idempotent, an append is not, and both spell the same
            // `prepare_block`. So the author declares it, here, at the one
            // place the operation is constructed. Required, not defaulted:
            // a default in either direction is the compiler guessing.
            Some("idempotent") => {
                if idempotent.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `idempotent=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `idempotent=` is a declaration, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let Expr::Bool(_, v) = &arg.value else {
                    return Err(type_error(
                        "`prepare_block`'s `idempotent=` is a declaration the operation's type \
                         carries, so it must be the literal `true` or `false` \
                         (03-hardware.md §9)"
                            .to_string(),
                        arg.span,
                    ));
                };
                idempotent = Some(*v);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`prepare_block`'s own arguments are labelled `permit=`, `header=`, \
                         `payload=`, `device_writes_payload=`, `status=`, `idempotent=`; \
                         `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`prepare_block(...)` requires labelled arguments".to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (
        Some(permit),
        Some(header),
        Some(payload),
        Some(device_writes),
        Some(status),
        Some(idempotent),
    ) = (permit, header, payload, device_writes, status, idempotent)
    else {
        return Err(type_error(
            "`prepare_block` needs `permit=`, `header=`, `payload=`, `device_writes_payload=`, \
             `status=` and `idempotent=`"
                .to_string(),
            call_span,
        ));
    };
    if !idempotent && fctx.in_unknown_outcome_arm() {
        return Err(type_error(
            no_auto_retry_message("prepare_block"),
            call_span,
        ));
    }
    let payload_ty = payload.ty.clone();
    let _ = (fspan, &queue);
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named(
            "QueueOp".to_string(),
            vec![
                types::TypeArg::Type(payload_ty),
                // The declaration rides on the operation's *type*, so a
                // `publish` that never sees the `prepare_block` site (one
                // hoisted out of the arm, say) still knows the answer.
                // `Span::default()` keeps two identically-declared
                // operations structurally equal.
                types::TypeArg::Const(Expr::Bool(Span::default(), idempotent)),
            ],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.prepare_block".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: vec![
                ("permit".to_string(), permit),
                ("header".to_string(), header),
                ("payload".to_string(), payload),
                ("device_writes_payload".to_string(), device_writes),
                ("status".to_string(), status),
            ],
        },
    })
}

pub(crate) fn require_layout_dma(
    ty: &Type,
    role: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let Type::Named(name, targs) = ty else {
        return Err(type_error(
            format!(
                "`prepare_block`'s `{role}=` must be a `@layout(dma)` struct; found `{}`",
                types::render_type(ty)
            ),
            span,
        ));
    };
    if !targs.is_empty() {
        return Err(type_error(
            format!(
                "`prepare_block`'s `{role}=` must be a `@layout(dma)` struct; found `{}`",
                types::render_type(ty)
            ),
            span,
        ));
    }
    match mctx.layouts.get(name.as_str()) {
        Some(l) if l.kind == types::LayoutKind::Dma => Ok(()),
        _ => Err(type_error(
            format!("`prepare_block`'s `{role}=` must be a `@layout(dma)` struct; `{name}` is not"),
            span,
        )),
    }
}

/// `queue.publish(operation=take ...)` — 03-hardware.md §5 / decision 15:
/// writes the ring in normative order and yields `Receipt[P]` for the
/// packaged payload brand.
pub(crate) fn check_virtqueue_publish(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.publish(operation=take ...)` takes exactly one labelled argument; \
                 found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("operation") {
        return Err(type_error(
            "`VirtQueue.publish`'s own argument is labelled `operation=` (03-hardware.md §4/§5)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            "`publish` consumes the prepared operation: write `operation=take ...` \
             (03-hardware.md §4/§5)"
                .to_string(),
            arg.span,
        ));
    }
    let op = check_expr(&arg.value, None, fctx, mctx)?;
    // plans/M8.md item G, decision 18: the operation's own type carries the
    // author's idempotence declaration, so this catches a `prepare_block`
    // hoisted out of the arm just as surely as one written inside it.
    if let Type::Named(n, targs) = &op.ty {
        if n == "QueueOp"
            && matches!(
                targs.get(1),
                Some(types::TypeArg::Const(Expr::Bool(_, false)))
            )
            && fctx.in_unknown_outcome_arm()
        {
            return Err(type_error(no_auto_retry_message("publish"), call_span));
        }
    }
    let payload_ty = match &op.ty {
        Type::Named(n, targs) if n == "QueueOp" => match targs.first() {
            Some(types::TypeArg::Type(p)) => p.clone(),
            _ => {
                return Err(type_error(
                    "`publish`'s `operation=` is a `QueueOp[P]`; found a `QueueOp` with no \
                     payload brand"
                        .to_string(),
                    arg.span,
                ));
            }
        },
        other => {
            return Err(type_error(
                format!(
                    "`publish`'s `operation=` is a `QueueOp[P]` (03-hardware.md §4); found `{}`",
                    types::render_type(other)
                ),
                arg.span,
            ));
        }
    };
    let op = with_arg_mode(arg.mode, op, arg.span);
    let _ = (fspan, &queue);
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named(
            "Receipt".to_string(),
            vec![types::TypeArg::Type(payload_ty)],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.publish".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: vec![("operation".to_string(), op)],
        },
    })
}

/// `queue.reject(payload=take p, error=...)` — 03-hardware.md §5:
/// pre-commit failure returns `P` via a resolved receipt.
pub(crate) fn check_virtqueue_reject(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 2 {
        return Err(type_error(
            format!(
                "`VirtQueue.reject(payload=take ..., error=...)` takes exactly two labelled \
                 arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut payload = None;
    let mut error = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("payload") => {
                if payload.is_some() {
                    return Err(type_error(
                        "`reject`'s `payload=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`reject` returns the payload through the receipt: write \
                         `payload=take ...` (03-hardware.md §5)"
                            .to_string(),
                        arg.span,
                    ));
                }
                payload = Some(with_arg_mode(
                    arg.mode,
                    check_expr(&arg.value, None, fctx, mctx)?,
                    arg.span,
                ));
            }
            Some("error") => {
                if error.is_some() {
                    return Err(type_error(
                        "`reject`'s `error=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`reject`'s `error=` is an `IoError` value, not a moved handle: \
                             drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let expected = Type::Named("IoError".to_string(), vec![]);
                error = Some(check_expr(&arg.value, Some(&expected), fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`reject`'s own arguments are labelled `payload=` and `error=`; \
                         `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`reject(...)` requires labelled arguments".to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (Some(payload), Some(error)) = (payload, error) else {
        return Err(type_error(
            "`reject` needs `payload=` and `error=`".to_string(),
            call_span,
        ));
    };
    let _ = (fspan, &queue);
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named(
            "Receipt".to_string(),
            vec![types::TypeArg::Type(payload.ty.clone())],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.reject".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: vec![
                ("payload".to_string(), payload),
                ("error".to_string(), error),
            ],
        },
    })
}

/// `queue.drain(max=N)` — bounded used-ring walk (03-hardware.md §4/§6).
pub(crate) fn check_virtqueue_drain(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    let depth = virtqueue_type_depth(&queue.ty, mctx).ok_or_else(|| {
        type_error(
            "`drain` needs a `VirtQueue[..N]` whose depth is a comptime-known nonzero power of two"
                .to_string(),
            call_span,
        )
    })?;
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.drain(max=N)` takes exactly one labelled argument; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("max") {
        return Err(type_error(
            "`VirtQueue.drain`'s own argument is labelled `max=` (03-hardware.md §6)".to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`drain`'s `max=` is a bound, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let max_expr = check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?;
    let max_val = virtqueue_depth_value(&max_expr, mctx).ok_or_else(|| {
        type_error(
            "`drain`'s `max=` must be a comptime-known integer (03-hardware.md §6)".to_string(),
            arg.span,
        )
    })?;
    if max_val == 0 || max_val > depth {
        return Err(type_error(
            format!("`drain(max={max_val})` on `VirtQueue[..{depth}]`: max must be in 1..={depth}"),
            arg.span,
        ));
    }
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.drain".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: Some(Type::Named(
                "VirtQueue".to_string(),
                vec![types::TypeArg::Bound(Expr::Int(
                    call_span,
                    max_val.to_string(),
                ))],
            )),
            const_arg: None,
            args: vec![("max".to_string(), max_expr)],
        },
    })
}

/// `queue.claim(receipt=take r) -> IoCompletion[P]` — plans/M7.md item E4 /
/// decision 22: sync claim of a drain-resolved receipt (bottom-half dual
/// of `await receipt`).
pub(crate) fn check_virtqueue_claim(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.claim(receipt=take ...)` takes exactly one labelled argument; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("receipt") {
        return Err(type_error(
            "`VirtQueue.claim`'s own argument is labelled `receipt=` (plans/M7.md item E4)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            "`claim` consumes the receipt: write `receipt=take ...` (03-hardware.md §5)"
                .to_string(),
            arg.span,
        ));
    }
    let receipt = check_expr(&arg.value, None, fctx, mctx)?;
    let Type::Named(n, targs) = &receipt.ty else {
        return Err(type_error(
            format!(
                "`claim`'s `receipt=` must be a `Receipt[P]`; found `{}`",
                types::render_type(&receipt.ty)
            ),
            arg.span,
        ));
    };
    if n != "Receipt" {
        return Err(type_error(
            format!(
                "`claim`'s `receipt=` must be a `Receipt[P]`; found `{}`",
                types::render_type(&receipt.ty)
            ),
            arg.span,
        ));
    }
    let Some(types::TypeArg::Type(payload)) = targs.first() else {
        return Err(type_error(
            "`Receipt` with no payload type argument".to_string(),
            arg.span,
        ));
    };
    let payload = payload.clone();
    let receipt = with_arg_mode(arg.mode, receipt, arg.span);
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named(
            "IoCompletion".to_string(),
            vec![types::TypeArg::Type(payload)],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.claim".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: vec![("receipt".to_string(), receipt)],
        },
    })
}

/// `queue.recover(receipt=take r) -> CompletionOutcome` — plans/M8.md item
/// G / decision 12: 03-hardware.md §5's `Recovery` transition, and the one
/// producer of §9's `CompletionOutcome`.
///
/// **Why this is not a second `claim`.** `claim` is the *resolved* path: it
/// consumes the receipt and returns the payload with the completion, which
/// is only sound because the device provably returned the descriptor in the
/// current epoch. `recover` is the *abandon* path §9 describes ("cancelling
/// in-flight work is a driver protocol, not a dropped future"): it consumes
/// the receipt — receipts resolve exactly once and dropping one is illegal
/// in every state (§5) — reports what is known about the operation's effect,
/// and deliberately returns **no payload**, because after a reset the buffer
/// is possibly device-owned and §9 forbids reclaiming it. Reclaim is
/// quarantine's job (plans/M8.md item F); until it lands the pool slot is
/// simply retired, which is the fail-closed half of the same sentence.
pub(crate) fn check_virtqueue_recover(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.recover(receipt=take ...)` takes exactly one labelled argument; \
                 found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("receipt") {
        return Err(type_error(
            "`VirtQueue.recover`'s own argument is labelled `receipt=` (03-hardware.md §5)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            "`recover` consumes the receipt: write `receipt=take ...` (03-hardware.md §5: \
             a receipt resolves exactly once)"
                .to_string(),
            arg.span,
        ));
    }
    let receipt = check_expr(&arg.value, None, fctx, mctx)?;
    match &receipt.ty {
        Type::Named(n, _) if n == "Receipt" => {}
        other => {
            return Err(type_error(
                format!(
                    "`recover`'s `receipt=` must be a `Receipt[P]`; found `{}`",
                    types::render_type(other)
                ),
                arg.span,
            ));
        }
    }
    // plans/M8.md item H attack 1: remember the receipt's `own[P] T` brand
    // on this queue place so a later `reclaim` cannot declare a different
    // pool and mint a confused handle.
    if let Some(key) = virtqueue_place_key(&queue) {
        if let Some(brand) = receipt_own_brand(&receipt.ty) {
            fctx.quarantined_by_queue.insert(key, brand);
        }
    }
    let receipt = with_arg_mode(arg.mode, receipt, arg.span);
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Named("CompletionOutcome".to_string(), vec![]),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.recover".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: vec![("receipt".to_string(), receipt)],
        },
    })
}

/// `queue.reclaim(pool=P, payload=T) -> own[P] T` — plans/M8.md item F /
/// **decision 37**: 03-hardware.md §9's "affected regions and DMA slots
/// are quarantined, per-queue reset ... or full reset establishes
/// quiescence, and **only then is memory reclaimed**".
///
/// **Why two declaring arguments and no receipt.** `recover` already
/// consumed the receipt (§5: a receipt resolves exactly once, and dropping
/// one is illegal in every state), and with it the only value that carried
/// the payload's brand — so the handle's type has to be *declared* here,
/// exactly as `img.dma_pool[T](name=P, ...)` declares the same pair when
/// the pool is created. Both arguments are bare names with no value form:
/// `pool=` is a bound pool name (02-language.md §4) and `payload=` names
/// the `@layout(dma)` struct the slot holds. They are resolved through the
/// ordinary `own[P] T` resolver, so an undeclared pool and a non-`dma`
/// payload are the same two diagnostics they are in any annotation.
///
/// **What the declaration cannot lie about.** The address handed back is
/// the quarantined slot's own payload word, so the *bytes* are always the
/// abandoned buffer's; the declaration decides which pool the language
/// believes the handle belongs to. plans/M8.md item H attack 1 closes the
/// pool-brand half at build time: `pool=`/`payload=` must match the
/// `own[P] T` brand of the `recover` that quarantined this queue's slot in
/// the same function (same `match` arm). A wrong brand would otherwise
/// survive any path that never reaches a later `publish`/`Receipt` store.
/// Checking the handle against the queue's *device* stays the deliberate
/// trade item P recorded as decision 27 — that is a different sentence.
pub(crate) fn check_virtqueue_reclaim(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    let (pool, payload) = reclaim_declaration(args, call_span)?;
    // Shape first: undeclared pool / non-dma payload keep the diagnostics
    // they have in any `own[P] T` annotation (`golden/err-reclaim-payload-not-dma`).
    let ast_ty = ast::Type::Own(Box::new(ast::OwnType {
        span: call_span,
        pool: vec![pool.1.clone()],
        inner: ast::Type::Named(NamedType {
            span: payload.0,
            name: payload.1.clone(),
            args: vec![],
        }),
    }));
    let ty = mctx.resolve_type(&ast_ty, &fctx.local_pools)?;
    match mctx.layouts.get(payload.1.as_str()) {
        Some(l) if l.kind == types::LayoutKind::Dma => {}
        _ => {
            return Err(type_error(
                format!(
                    "`reclaim`'s `payload=` must be a `@layout(dma)` struct; `{}` is not \
                     (03-hardware.md §3: a transfer payload is `own[P] T` where `T` is \
                     `@layout(dma)`)",
                    payload.1
                ),
                payload.0,
            ));
        }
    }
    // Brand second (plans/M8.md item H attack 1): the declaration must
    // match the `own[P] T` `recover` quarantined on this queue place.
    let Some(key) = virtqueue_place_key(&queue) else {
        return Err(type_error(
            "`reclaim` needs a named `VirtQueue` place (a local or a field) so its \
             `pool=` can be checked against the brand `recover` quarantined on that \
             queue (plans/M8.md item H; 04-compiler.md §1: DMA ownership transitions \
             are valid)"
                .to_string(),
            call_span,
        ));
    };
    let Some((expected_pool, expected_payload)) = fctx.quarantined_by_queue.remove(&key) else {
        return Err(type_error(
            "`reclaim` on this queue has no preceding `recover` in this scope whose \
             receipt brands a pool; write `recover` first, then \
             `reclaim(pool=<that brand>, payload=...)` (plans/M8.md item H / \
             03-hardware.md §9)"
                .to_string(),
            call_span,
        ));
    };
    if pool.1 != expected_pool {
        return Err(type_error(
            format!(
                "`reclaim`'s `pool={}` does not match the pool brand recovered on this \
                 queue (`{expected_pool}`); the handle would be `own[{}]` pointing at \
                 `{expected_pool}`'s bytes (03-hardware.md §9 / 04-compiler.md §1: DMA \
                 ownership transitions are valid)",
                pool.1, pool.1
            ),
            pool.0,
        ));
    }
    if payload.1 != expected_payload {
        return Err(type_error(
            format!(
                "`reclaim`'s `payload={}` does not match the payload type recovered on \
                 this queue (`{expected_payload}`) (03-hardware.md §9)",
                payload.1
            ),
            payload.0,
        ));
    }
    Ok(TypedExpr {
        span: call_span,
        ty,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.reclaim".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
    })
}

/// Place key for a `VirtQueue` receiver — a local name, or `root.field`
/// for a field of a local (the `self.queue` spelling every flagship uses).
pub(crate) fn virtqueue_place_key(queue: &TypedExpr) -> Option<String> {
    match &queue.kind {
        TypedExprKind::Local(n) => Some(n.clone()),
        TypedExprKind::Field(base, field) => match &base.kind {
            TypedExprKind::Local(root) => Some(format!("{root}.{field}")),
            _ => virtqueue_place_key(base).map(|p| format!("{p}.{field}")),
        },
        _ => None,
    }
}

/// `Receipt[own[P] T]` → `(P, T)` — the brand `recover` quarantines and
/// `reclaim` must re-declare. Anything else yields `None` (a receipt that
/// does not carry an `own` payload cannot justify a reclaim brand).
pub(crate) fn receipt_own_brand(ty: &Type) -> Option<(String, String)> {
    let Type::Named(n, args) = ty else {
        return None;
    };
    if n != "Receipt" {
        return None;
    }
    let Some(types::TypeArg::Type(Type::Own(pool, inner))) = args.first() else {
        return None;
    };
    match inner.as_ref() {
        Type::Named(payload, _) => Some((pool.clone(), payload.clone())),
        _ => None,
    }
}

/// The `pool=P, payload=T` pair `reclaim` declares, as two bare names.
/// Shared by `bodies` (which resolves them) and `access` (which only needs
/// the shape to keep a move tracked), so the two passes cannot disagree
/// about what a well-formed `reclaim` looks like.
pub(crate) fn reclaim_declaration(
    args: &[Arg],
    call_span: Span,
) -> Result<((Span, String), (Span, String)), SemaError> {
    let mut pool: Option<(Span, String)> = None;
    let mut payload: Option<(Span, String)> = None;
    for a in args {
        let slot = match a.label.as_deref() {
            Some("pool") => &mut pool,
            Some("payload") => &mut payload,
            _ => {
                return Err(type_error(
                    "`VirtQueue.reclaim(pool=..., payload=...)` takes exactly those two \
                     labelled arguments (03-hardware.md §9)"
                        .to_string(),
                    a.span,
                ));
            }
        };
        if slot.is_some() {
            return Err(type_error(
                format!(
                    "duplicate `{}=` argument",
                    a.label.as_deref().unwrap_or("?")
                ),
                a.span,
            ));
        }
        if a.mode != AccessMode::Read {
            return Err(type_error(
                "`reclaim`'s `pool=`/`payload=` are declarations, not values: they take no \
                 access mode"
                    .to_string(),
                a.span,
            ));
        }
        match &a.value {
            Expr::Name(span, name) => *slot = Some((*span, name.clone())),
            other => {
                return Err(type_error(
                    "`reclaim`'s `pool=`/`payload=` are bare names — a declared pool and a \
                     `@layout(dma)` struct"
                        .to_string(),
                    other.span(),
                ));
            }
        }
    }
    match (pool, payload) {
        (Some(p), Some(t)) => Ok((p, t)),
        _ => Err(type_error(
            "`VirtQueue.reclaim(pool=..., payload=...)` needs both: the pool the quarantined \
             slot belongs to and the `@layout(dma)` payload it holds (03-hardware.md §9)"
                .to_string(),
            call_span,
        )),
    }
}

/// `queue.suppress_interrupts()` — set `VIRTQ_AVAIL_F_NO_INTERRUPT` (poll builds).
pub(crate) fn check_virtqueue_suppress_interrupts(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    _fctx: &mut FnCtx,
    _mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`VirtQueue.suppress_interrupts()` takes no arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.suppress_interrupts".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
    })
}

/// Depth bound on a `VirtQueue[..N]` type, resolving a const name through
/// `mctx.const_values` the same way `virtqueue_depth_value` does for a
/// typed expression.
pub(crate) fn virtqueue_type_depth(ty: &Type, mctx: &ModuleCtx) -> Option<u64> {
    let Type::Named(name, targs) = ty else {
        return None;
    };
    if name != "VirtQueue" {
        return None;
    }
    let types::TypeArg::Bound(expr) = targs.first()? else {
        return None;
    };
    match expr {
        Expr::Int(_, text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
        Expr::Name(_, n) => {
            let init = mctx.const_values.get(n)?;
            match init {
                Expr::Int(_, text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `VirtQueue.configure(pool=take control_pool, device=mut negotiated,
/// index=0, depth=QDEPTH)?` — FeaturesAccepted -> QueuesConfigured, and
/// the `DmaShared` mint item D left named (03-hardware.md §3/§4).
pub(crate) fn check_virtqueue_configure(
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 4 {
        return Err(type_error(
            format!(
                "`VirtQueue.configure(pool=take ..., device=mut ..., index=..., depth=...)` \
                 takes exactly four labelled arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut pool = None;
    let mut device = None;
    let mut device_local: Option<String> = None;
    let mut index = None;
    let mut depth = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("pool") => {
                if pool.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `pool=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`VirtQueue.configure` consumes the DMA pool: write `pool=take ...` \
                         (03-hardware.md §3: the queue owns the shared control memory minted \
                         out of it)"
                            .to_string(),
                        arg.span,
                    ));
                }
                pool = Some(with_arg_mode(
                    arg.mode,
                    check_expr(&arg.value, None, fctx, mctx)?,
                    arg.span,
                ));
            }
            Some("device") => {
                if device.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `device=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Mut {
                    return Err(type_error(
                        "`VirtQueue.configure` takes the device by `mut` so the local becomes \
                         `QueuesConfiguredDevice[D]` after the call (03-hardware.md §9): write \
                         `device=mut ...`"
                            .to_string(),
                        arg.span,
                    ));
                }
                if let Expr::Name(_, n) = &arg.value {
                    device_local = Some(n.clone());
                }
                device = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some("index") => {
                if index.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `index=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`VirtQueue.configure`'s `index=` is a queue index, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                index = Some(check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?);
            }
            Some("depth") => {
                if depth.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `depth=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`VirtQueue.configure`'s `depth=` is a queue depth, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                depth = Some(check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`VirtQueue.configure`'s own arguments are labelled `pool=`, `device=`, \
                         `index=`, `depth=`; `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`VirtQueue.configure(...)` requires labelled arguments \
                     (docs/language/examples/virtio-storage.wr)"
                        .to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (Some(pool), Some(device_expr), Some(index), Some(depth_expr)) =
        (pool, device, index, depth)
    else {
        return Err(type_error(
            "`VirtQueue.configure` needs `pool=`, `device=`, `index=` and `depth=`".to_string(),
            call_span,
        ));
    };
    // Pool must be a DmaPool[P, N].
    let pool_ty = unwrap_own(pool.ty.clone());
    let Type::Named(pool_name, pool_targs) = &pool_ty else {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `pool=` is a `DmaPool[P, N]`; found `{}`",
                types::render_type(&pool.ty)
            ),
            call_span,
        ));
    };
    if pool_name != "DmaPool" {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `pool=` is a `DmaPool[P, N]`; found `{}`",
                types::render_type(&pool.ty)
            ),
            call_span,
        ));
    }
    let Some(types::TypeArg::Pool(pool_id)) = pool_targs.first() else {
        return Err(type_error(
            "`VirtQueue.configure`'s `DmaPool` names no pool".to_string(),
            call_span,
        ));
    };
    // Device must be FeaturesAcceptedDevice[D].
    let device_ty = unwrap_own(device_expr.ty.clone());
    let Type::Named(dev_state, dev_targs) = &device_ty else {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `device=` is a `FeaturesAcceptedDevice[D]`; found `{}`",
                types::render_type(&device_expr.ty)
            ),
            call_span,
        ));
    };
    if dev_state != "FeaturesAcceptedDevice" {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `device=` is a `FeaturesAcceptedDevice[D]` \
                 (03-hardware.md §9: FeaturesAccepted -> QueuesConfigured); found `{}`",
                types::render_type(&device_expr.ty)
            ),
            call_span,
        ));
    }
    let device_name = device_type_arg(dev_targs).unwrap_or("?").to_string();
    // Depth must be a comptime-known nonzero power of two. Prefer a
    // literal; a module const name is accepted when its value is a
    // literal int (the common `const QDEPTH: usize = 128` spelling).
    let depth_val = virtqueue_depth_value(&depth_expr, mctx).ok_or_else(|| {
        type_error(
            "`VirtQueue.configure`'s `depth=` must be a comptime-known nonzero power of two \
             (VIRTIO 1.2 §2.6); a runtime value would make the ring geometry — which the \
             report, the placer and the VMM all read from one derivation — disagree with \
             itself"
                .to_string(),
            call_span,
        )
    })?;
    if depth_val == 0 || !depth_val.is_power_of_two() || depth_val > u16::MAX as u64 {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `depth={depth_val}` is not a nonzero power of two that \
                 fits virtio's 16-bit queue depth (VIRTIO 1.2 §2.6)"
            ),
            call_span,
        ));
    }
    // index must be 0 on machine v1 (one queue).
    if let TypedExprKind::Int(text) = &index.kind {
        if let Some(v) = parse_int_literal(text) {
            if v != 0 {
                return Err(type_error(
                    format!(
                        "`VirtQueue.configure`'s `index={v}`: machine v1's `blk` has exactly one \
                         queue (index 0)"
                    ),
                    call_span,
                ));
            }
        }
    }
    // Flow-type the mut device local to QueuesConfiguredDevice[D].
    if let Some(local) = &device_local {
        let queued = device_state_ty("QueuesConfiguredDevice", &device_name);
        if !fctx.retype_local(local, queued) {
            return Err(type_error(
                "`VirtQueue.configure`'s `device=mut ...` must name a local so its type can \
                 become `QueuesConfiguredDevice[D]` after the call (03-hardware.md §9)"
                    .to_string(),
                call_span,
            ));
        }
    } else {
        return Err(type_error(
            "`VirtQueue.configure`'s `device=mut ...` must name a local so its type can \
             become `QueuesConfiguredDevice[D]` after the call (03-hardware.md §9)"
                .to_string(),
            call_span,
        ));
    }
    let _ = (fspan, pool_id);
    // Record for layout/report: one derivation of (pool, depth).
    mctx.virtqueue_configures
        .borrow_mut()
        .push((pool_id.clone(), depth_val as u16));
    let queue_ty = Type::Named(
        "VirtQueue".to_string(),
        vec![types::TypeArg::Bound(Expr::Int(
            call_span,
            depth_val.to_string(),
        ))],
    );
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Result(Box::new(queue_ty), Box::new(boot_error_ty())),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.configure".to_string(),
            receiver: None,
            type_arg: Some(Type::Named(device_name, vec![])),
            const_arg: None,
            args: vec![
                ("pool".to_string(), pool),
                ("device".to_string(), device_expr),
                ("index".to_string(), index),
                ("depth".to_string(), depth_expr),
            ],
        },
    })
}

/// A comptime depth for `VirtQueue.configure`: a literal int, or a
/// module `const` whose initializer is a literal int.
pub(crate) fn virtqueue_depth_value(expr: &TypedExpr, mctx: &ModuleCtx) -> Option<u64> {
    match &expr.kind {
        TypedExprKind::Int(text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
        TypedExprKind::Const(name) => {
            let init = mctx.const_values.get(name)?;
            match init {
                Expr::Int(_, text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// plans/M7.md item G: `IrqCap.bind` / `IrqCap.unmask` — the two
/// operations 03-hardware.md §6's worked example names on an `IrqCap`.
pub fn is_irq_cap_intrinsic(key: &str) -> bool {
    matches!(key, "IrqCap.bind" | "IrqCap.unmask")
}

/// plans/M7.md item G, decision 17: `InterruptCell[T]` ops + constructor.
pub fn is_interrupt_cell_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "InterruptCell.new"
            | "InterruptCell.load_acquire"
            | "InterruptCell.store_release"
            | "InterruptCell.swap_acquire"
            | "InterruptCell.fetch_or_release"
    )
}

/// plans/M7.md item G: `wake(Driver.method)`.
pub fn is_wake_intrinsic(key: &str) -> bool {
    key == "wake"
}

/// Is `ty` an `InterruptCell[_]`?
pub fn is_interrupt_cell_type(ty: &Type) -> bool {
    matches!(unwrap_own(ty.clone()), Type::Named(n, _) if n == "InterruptCell")
}
