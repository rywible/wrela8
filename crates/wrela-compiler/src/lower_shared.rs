use crate::mwir::{Inst, Temp};
use crate::sema::bodies;
use crate::sema::typed::{TypedExpr, TypedExprKind, TypedProgram};
use crate::sema::types::{LayoutField, Type};

pub fn runtime_layout_field_offset(
    prog: &TypedProgram,
    layout: &str,
    field: &str,
) -> Result<u64, String> {
    Ok(runtime_layout_field(prog, layout, field)?.offset)
}

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
        .and_then(|l| l.size)
}

pub fn placed_array_field_index(
    array_place: &TypedExpr,
    prog: &TypedProgram,
    eval_len: impl FnOnce(&Type) -> Result<usize, String>,
) -> Result<Option<(TypedExpr, u64, u64, usize)>, String> {
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

pub fn placed_struct_array_scalar_field(
    elem_place: &TypedExpr,
    field_name: &str,
    prog: &TypedProgram,
    eval_len: impl FnOnce(&Type) -> Result<usize, String>,
) -> Result<Option<(TypedExpr, TypedExpr, u64, u64, usize)>, String> {
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

pub fn emit_collapse_reserve_permit(dst: Temp, src: Temp, mut emit: impl FnMut(Inst)) {
    emit(Inst::EnumPayload { dst, src, index: 0 });
}

pub struct PrepareBlockParts<'a> {
    pub permit: &'a TypedExpr,
    pub header: &'a TypedExpr,
    pub payload: &'a TypedExpr,
    pub status: &'a TypedExpr,
    pub device_writes: bool,
}

pub enum PrepareBlockUnpackError {
    Missing(&'static str),
    NonLiteralDeviceWrites,
}

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
