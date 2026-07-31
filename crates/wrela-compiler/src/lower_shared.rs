//! Shared Inst-producing helpers for `lower` and `flowwir_lower`.
//!
//! Both lowerers emit the same `mwir::Inst` shapes for placed-static
//! field access, reserve-permit collapse, and `VirtQueue.prepare_block`
//! unpacking. This module owns that logic once; each caller supplies a
//! tiny local emitter surface (`fresh` / `emit` callbacks) — no traits
//! for backends.

use crate::mwir::{Inst, Temp};
use crate::sema::bodies;
use crate::sema::typed::{TypedExpr, TypedExprKind, TypedProgram};
use crate::sema::types::{LayoutField, Type};

/// Declared offset of `field` in a `@layout(runtime)` type — same table
/// `check_layouts` produced.
pub fn runtime_layout_field_offset(
    prog: &TypedProgram,
    layout: &str,
    field: &str,
) -> Result<u64, String> {
    Ok(runtime_layout_field(prog, layout, field)?.offset)
}

/// Offset and dense byte size of a `@layout(runtime)` field. The `size`
/// is `len * elem_stride` for array fields.
pub fn runtime_layout_field(
    prog: &TypedProgram,
    layout: &str,
    field: &str,
) -> Result<LayoutField, String> {
    let Some(l) = prog.layouts.iter().find(|l| l.name == layout) else {
        return Err(format!(
            "placed-static field access through `{layout}`, which has no layout table entry"
        ));
    };
    for e in &l.entries {
        if let crate::sema::types::LayoutEntry::Field(f) = e {
            if f.name == field {
                return Ok(f.clone());
            }
        }
    }
    Err(format!(
        "`{layout}` declares no field `{field}` (the checker already refused this)"
    ))
}

/// Exact `@layout(dma)` byte size of an `own[P] T` payload (or bare `T`).
/// Used by `prepare_block` for the descriptor length — mwir `size_of` is
/// the frame ABI (8-byte slots), not the device-visible layout.
pub fn layout_dma_size(ty: &Type, prog: &TypedProgram) -> Option<u64> {
    let name = match ty {
        Type::Own(_, inner) => match inner.as_ref() {
            Type::Named(n, args) if args.is_empty() => n.as_str(),
            _ => return None,
        },
        Type::Named(n, args) if args.is_empty() => n.as_str(),
        _ => return None,
    };
    prog.layouts
        .iter()
        .find(|l| l.name == name && matches!(l.kind, crate::sema::types::LayoutKind::Dma))
        // `None` also covers a layout whose sizing is
        // still deferred, which is the same answer this fn already gives for
        // a missing layout.
        .and_then(|l| l.size)
}

/// `STATIC.array_field[i]` place parts: layout field offset, dense element
/// stride, and compile-time length.
pub fn placed_array_field_index(
    array_place: &TypedExpr,
    prog: &TypedProgram,
    eval_len: impl FnOnce(&Type) -> Result<usize, String>,
) -> Result<Option<(TypedExpr /* static base */, u64, u64, usize)>, String> {
    let TypedExprKind::Field(static_base, fname) = &array_place.kind else {
        return Ok(None);
    };
    let TypedExprKind::Static(sname) = &static_base.kind else {
        return Ok(None);
    };
    let layout_name = match bodies::unwrap_own(static_base.ty.clone()) {
        Type::Named(n, _) => n,
        other => {
            return Err(format!(
                "placed static `{sname}` has non-named type {other:?}"
            ));
        }
    };
    let field = runtime_layout_field(prog, &layout_name, fname)?;
    let len = eval_len(&array_place.ty)?;
    if len == 0 {
        return Err(format!(
            "placed array field `{layout_name}.{fname}` has length 0"
        ));
    }
    if field.size % len as u64 != 0 {
        return Err(format!(
            "placed array field `{layout_name}.{fname}` size {} is not divisible by len {len}",
            field.size
        ));
    }
    let elem_stride = field.size / len as u64;
    Ok(Some((
        (**static_base).clone(),
        field.offset,
        elem_stride,
        len,
    )))
}

/// `STATIC.struct_array[i].field` — scalar through a placed array of
/// `@layout(runtime)` structs.
pub fn placed_struct_array_scalar_field(
    elem_place: &TypedExpr,
    field_name: &str,
    prog: &TypedProgram,
    eval_len: impl FnOnce(&Type) -> Result<usize, String>,
) -> Result<
    Option<(
        TypedExpr, /* static */
        TypedExpr, /* index */
        u64,
        u64,
        usize,
    )>,
    String,
> {
    let TypedExprKind::Index(array_place, idx_expr) = &elem_place.kind else {
        return Ok(None);
    };
    let Some((static_expr, array_off, elem_stride, len)) =
        placed_array_field_index(array_place, prog, eval_len)?
    else {
        return Ok(None);
    };
    let elem_layout = match bodies::unwrap_own(elem_place.ty.clone()) {
        Type::Named(n, _) => n,
        other => {
            return Err(format!(
                "placed struct-array element has non-named type {other:?}"
            ));
        }
    };
    let sub = runtime_layout_field_offset(prog, &elem_layout, field_name)?;
    Ok(Some((
        static_expr,
        (**idx_expr).clone(),
        array_off + sub,
        elem_stride,
        len,
    )))
}

/// When a use site coerced `Result[QueuePermit, CapacityError]` to
/// `QueuePermit`, the typed node carries the success type but the temp
/// still holds a Result — project the Ok payload. Returns `true` when
/// the caller should emit `EnumPayload { index: 0 }` into a fresh temp
/// of `expr_ty` (tiny predicate surface — each lowerer owns its own
/// `fresh`/`emit`).
pub fn needs_collapse_reserve_permit(expr_ty: &Type, src_ty: &Type) -> bool {
    let is_permit = matches!(expr_ty, Type::Named(n, t) if n == "QueuePermit" && t.is_empty());
    if !is_permit {
        return false;
    }
    match src_ty {
        Type::Result(ok, err) => {
            matches!(&**ok, Type::Named(n, t) if n == "QueuePermit" && t.is_empty())
                && matches!(&**err, Type::Named(n, t) if n == "CapacityError" && t.is_empty())
        }
        _ => false,
    }
}

/// Emit the Ok-payload projection for a collapsed reserve permit.
pub fn emit_collapse_reserve_permit(dst: Temp, src: Temp, mut emit: impl FnMut(Inst)) {
    emit(Inst::EnumPayload { dst, src, index: 0 });
}

/// Labelled `prepare_block` operands after the checker has accepted them.
pub struct PrepareBlockParts<'a> {
    pub permit: &'a TypedExpr,
    pub header: &'a TypedExpr,
    pub payload: &'a TypedExpr,
    pub status: &'a TypedExpr,
    pub device_writes: bool,
}

/// Why unpacking `prepare_block` labelled args failed.
pub enum PrepareBlockUnpackError {
    Missing(&'static str),
    NonLiteralDeviceWrites,
}

/// Unpack `permit=`/`header=`/`payload=`/`status=`/`device_writes_payload=`
/// from a `VirtQueue.prepare_block` call site.
pub fn unpack_prepare_block_args(
    args: &[(String, TypedExpr)],
) -> Result<PrepareBlockParts<'_>, PrepareBlockUnpackError> {
    let permit = args
        .iter()
        .find(|(l, _)| l == "permit")
        .ok_or(PrepareBlockUnpackError::Missing("permit="))?;
    let header = args
        .iter()
        .find(|(l, _)| l == "header")
        .ok_or(PrepareBlockUnpackError::Missing("header="))?;
    let payload = args
        .iter()
        .find(|(l, _)| l == "payload")
        .ok_or(PrepareBlockUnpackError::Missing("payload="))?;
    let status = args
        .iter()
        .find(|(l, _)| l == "status")
        .ok_or(PrepareBlockUnpackError::Missing("status="))?;
    let device_writes_arg = args
        .iter()
        .find(|(l, _)| l == "device_writes_payload")
        .ok_or(PrepareBlockUnpackError::Missing("device_writes_payload="))?;
    let device_writes = match &device_writes_arg.1.kind {
        TypedExprKind::Bool(v) => *v,
        _ => return Err(PrepareBlockUnpackError::NonLiteralDeviceWrites),
    };
    Ok(PrepareBlockParts {
        permit: &permit.1,
        header: &header.1,
        payload: &payload.1,
        status: &status.1,
        device_writes,
    })
}

/// Validate `@layout(dma)` payload length for virtio-blk (positive multiple
/// of 512). Returns the length on success.
pub fn prepare_block_payload_len(
    payload_ty: &Type,
    prog: &TypedProgram,
) -> Result<u64, PreparePayloadLenError> {
    let payload_len = layout_dma_size(payload_ty, prog).ok_or(PreparePayloadLenError::NoDmaSize)?;
    if payload_len == 0 || payload_len % 512 != 0 {
        return Err(PreparePayloadLenError::BadSectorMultiple(payload_len));
    }
    Ok(payload_len)
}

pub enum PreparePayloadLenError {
    NoDmaSize,
    BadSectorMultiple(u64),
}
