use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::sema::types::{self, Type};
use crate::syntax::ast::BinOp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Temp(pub usize);

impl std::fmt::Display for Temp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MwirProgram {
    pub fns: BTreeMap<String, MwirFn>,
    pub rodata: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MwirFn {
    pub receiver: Option<(Temp, crate::syntax::ast::AccessMode)>,
    pub params: Vec<(Temp, crate::syntax::ast::AccessMode)>,
    pub ret: Type,
    pub temp_types: Vec<Type>,
    pub body: Vec<Inst>,
}

impl MwirFn {
    pub fn temp_count(&self) -> usize {
        self.temp_types.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inst {
    ConstInt {
        dst: Temp,
        ty: Type,
        value: i128,
    },
    ConstBool {
        dst: Temp,
        value: bool,
    },
    ConstFloat {
        dst: Temp,
        ty: Type,
        bits: u64,
    },
    ConstChar {
        dst: Temp,
        value: char,
    },
    ConstUnit {
        dst: Temp,
    },
    ConstText {
        dst: Temp,
        data: usize,
    },

    Copy {
        dst: Temp,
        src: Temp,
    },

    MakeAggregate {
        dst: Temp,
        elems: Vec<Temp>,
    },
    FormatScalar {
        dst: Temp,
        src: Temp,
        src_ty: Type,
        capacity: usize,
    },
    StringConcat {
        dst: Temp,
        lhs: Temp,
        rhs: Temp,
        lhs_cap: usize,
        rhs_cap: usize,
    },
    Project {
        dst: Temp,
        base: Temp,
        index: usize,
    },
    SetField {
        base: Temp,
        index: usize,
        value: Temp,
    },

    IndexGet {
        dst: Temp,
        base: Temp,
        index: Temp,
        len: usize,
    },
    /// An index whose range analysis carries a checked, exact length proof.
    /// Only the proof-producing pass may construct this variant.
    IndexGetProven {
        dst: Temp,
        base: Temp,
        index: Temp,
        len: usize,
    },
    IndexSet {
        base: Temp,
        index: Temp,
        value: Temp,
        len: usize,
    },
    IndexSetProven {
        base: Temp,
        index: Temp,
        value: Temp,
        len: usize,
    },
    PlacedIndexGet {
        dst: Temp,
        base: Temp,
        field_offset: u64,
        index: Temp,
        len: usize,
        elem_stride: u64,
        ty: Type,
    },
    PlacedIndexGetProven {
        dst: Temp,
        base: Temp,
        field_offset: u64,
        index: Temp,
        len: usize,
        elem_stride: u64,
        ty: Type,
    },
    PlacedIndexSet {
        base: Temp,
        field_offset: u64,
        index: Temp,
        value: Temp,
        len: usize,
        elem_stride: u64,
        ty: Type,
    },
    PlacedIndexSetProven {
        base: Temp,
        field_offset: u64,
        index: Temp,
        value: Temp,
        len: usize,
        elem_stride: u64,
        ty: Type,
    },
    BytesIndexGet {
        dst: Temp,
        base: Temp,
        index: Temp,
    },

    MakeEnum {
        dst: Temp,
        tag: usize,
        payload: Vec<Temp>,
    },
    EnumTag {
        dst: Temp,
        src: Temp,
    },
    EnumPayload {
        dst: Temp,
        src: Temp,
        index: usize,
    },

    ArithChecked {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
        abort: String,
    },
    ArithWrapping {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
    },
    DivRem {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
        abort_zero: String,
        abort_overflow: String,
    },
    Shift {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
        bits: u32,
        lost: Option<String>,
    },
    Bitwise {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
    },
    Compare {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
    },
    Neg {
        dst: Temp,
        ty: Type,
        src: Temp,
        abort: String,
    },
    BitNot {
        dst: Temp,
        ty: Type,
        src: Temp,
    },
    Convert {
        dst: Temp,
        ty: Type,
        src: Temp,
        abort: String,
    },
    Not {
        dst: Temp,
        src: Temp,
    },
    BoolAnd {
        dst: Temp,
        lhs: Temp,
        rhs: Temp,
    },

    Jump {
        target: usize,
    },
    JumpIfFalse {
        cond: Temp,
        target: usize,
    },

    Call {
        dst: Temp,
        write_backs: Vec<(usize, Temp)>,
        key: String,
        args: Vec<Temp>,
    },
    Return {
        value: Option<Temp>,
    },

    MmioRead {
        dst: Temp,
        base: Temp,
        offset: u64,
        ty: Type,
    },
    MmioWrite {
        base: Temp,
        offset: u64,
        ty: Type,
        value: Temp,
    },
    LoadIrqVector {
        dst: Temp,
        driver: String,
    },
    InterruptCellLoadAcquire {
        dst: Temp,
        field_off: usize,
        width: u8,
    },
    InterruptCellStoreRelease {
        field_off: usize,
        width: u8,
        value: Temp,
    },
    InterruptCellSwapAcquire {
        dst: Temp,
        field_off: usize,
        width: u8,
        value: Temp,
    },
    InterruptCellFetchOrRelease {
        dst: Temp,
        field_off: usize,
        width: u8,
        value: Temp,
    },
    Dmb {
        option: String,
    },
    Wake {
        driver: String,
    },

    Now {
        dst: Temp,
    },
    Entropy {
        dst: Temp,
        n: u64,
    },

    SlotMapMint {
        map: Temp,
    },

    MemLoad {
        dst: Temp,
        base: Temp,
        offset: u64,
        width: u8,
    },
    MemStore {
        base: Temp,
        offset: u64,
        value: Temp,
        width: u8,
    },
    PtrOffset {
        dst: Temp,
        base: Temp,
        offset: u64,
    },
    TurnAddrFromId {
        dst: Temp,
        id: Temp,
    },
    Abort {
        message: String,
    },

    AssertFail {
        message: Option<String>,
    },
}

pub fn abort_message(op: BinOp) -> String {
    format!("arithmetic overflow in `{}`", op.as_str())
}

pub fn neg_abort_message() -> String {
    "arithmetic overflow in unary `-`".to_string()
}

pub fn div_zero_message(op: BinOp) -> String {
    format!(
        "{} by zero",
        if op == BinOp::Div {
            "division"
        } else {
            "remainder"
        }
    )
}

pub fn shift_lost_message() -> String {
    "`<<` lost nonzero high bits".to_string()
}

pub fn convert_abort_message(target: &Type) -> String {
    format!(
        "`.to[{}]()` conversion out of range",
        types::render_type(target)
    )
}

#[cfg(test)]
mod abort_message_tests {
    use super::*;
    use crate::eval::value::{self, Value};

    #[test]
    fn ordinary_overflow_wording_matches_the_evaluator() {
        let got = value::eval_ordinary(BinOp::Add, &Type::U8, &Value::U8(250), &Value::U8(10))
            .unwrap_err();
        assert_eq!(got, abort_message(BinOp::Add));
    }

    #[test]
    fn div_overflow_wording_matches_the_evaluator() {
        let got = value::eval_div_rem(
            BinOp::Div,
            &Type::I32,
            &Value::I32(i32::MIN),
            &Value::I32(-1),
        )
        .unwrap_err();
        assert_eq!(got, abort_message(BinOp::Div));
    }

    #[test]
    fn neg_overflow_wording_matches_the_evaluator() {
        let got = value::eval_neg(&Value::I8(i8::MIN)).unwrap_err();
        assert_eq!(got, neg_abort_message());
    }

    #[test]
    fn div_by_zero_wording_matches_the_evaluator() {
        let got = value::eval_div_rem(BinOp::Div, &Type::U32, &Value::U32(9), &Value::U32(0))
            .unwrap_err();
        assert_eq!(got, div_zero_message(BinOp::Div));
    }

    #[test]
    fn rem_by_zero_wording_matches_the_evaluator() {
        let got = value::eval_div_rem(BinOp::Rem, &Type::U32, &Value::U32(9), &Value::U32(0))
            .unwrap_err();
        assert_eq!(got, div_zero_message(BinOp::Rem));
    }

    #[test]
    fn shift_lost_bits_wording_matches_the_evaluator() {
        let got =
            value::eval_shift(BinOp::Shl, &Type::U8, &Value::U8(0xFF), &Value::U8(1)).unwrap_err();
        assert_eq!(got, shift_lost_message());
    }

    #[test]
    fn convert_out_of_range_wording_matches_the_evaluator() {
        let got = value::eval_to_scalar(&Type::U8, &Value::I32(-1)).unwrap_err();
        assert_eq!(got, convert_abort_message(&Type::U8));
    }
}

#[derive(Debug, Clone, Default)]
pub struct LayoutCtx {
    pub structs: BTreeMap<String, Vec<Type>>,
    pub enums: BTreeMap<String, Vec<Vec<Type>>>,
    pub struct_field_names: BTreeMap<String, Vec<String>>,
}

pub fn build_layout_ctx(
    module: &crate::syntax::ast::Module,
    imported: &types::ImportedTypes,
) -> Result<LayoutCtx, crate::sema::SemaError> {
    use crate::sema::types::{DeclEnum, DeclItem, DeclMember, DeclStruct, DeclVariantPayload};

    let specialized = crate::sema::specialize::specialize(module)?;
    let mut imported = imported.clone();
    if crate::loader::module_mentions_time(module) {
        for name in ["Duration", "Instant"] {
            imported.entry(name.to_string()).or_insert(0);
        }
    }
    let items = types::declare_with_imports(&specialized, &imported)?;
    let mut ctx = LayoutCtx::default();
    for item in items {
        match item {
            DeclItem::Struct(DeclStruct { name, members, .. }) => {
                let field_names: Vec<String> = members
                    .iter()
                    .filter_map(|m| match m {
                        DeclMember::Field(f) => Some(f.name.clone()),
                        _ => None,
                    })
                    .collect();
                let fields: Vec<Type> = members
                    .into_iter()
                    .filter_map(|m| match m {
                        DeclMember::Field(f) => Some(f.ty),
                        _ => None,
                    })
                    .collect();
                ctx.struct_field_names.insert(name.clone(), field_names);
                ctx.structs.insert(name, fields);
            }
            DeclItem::Enum(DeclEnum { name, variants, .. }) => {
                let payloads: Vec<Vec<Type>> = variants
                    .into_iter()
                    .map(|v| match v.payload {
                        DeclVariantPayload::None => Vec::new(),
                        DeclVariantPayload::Tuple(tys) => tys,
                        DeclVariantPayload::Named(fields) => {
                            fields.into_iter().map(|(_, t)| t).collect()
                        }
                    })
                    .collect();
                ctx.enums.insert(name, payloads);
            }
            _ => {}
        }
    }
    Ok(ctx)
}

pub(crate) fn io_completion_fields(
    targs: &[crate::sema::types::TypeArg],
) -> Result<[(&'static str, Type); 3], String> {
    let Some(crate::sema::types::TypeArg::Type(payload)) = targs.first() else {
        return Err("`IoCompletion` with no payload type argument".to_string());
    };
    Ok([
        ("payload", payload.clone()),
        (
            "status",
            Type::Result(
                Box::new(Type::Unit),
                Box::new(Type::Named("IoError".to_string(), vec![])),
            ),
        ),
        (
            "written_len",
            Type::Named(
                "Untrusted".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Usize)],
            ),
        ),
    ])
}

pub fn size_of(ty: &Type, ctx: &LayoutCtx) -> Result<usize, String> {
    const SLOT: usize = 8;
    match ty {
        Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::Unit
        | Type::Never => Ok(SLOT),
        Type::Array(elem, len_expr) => {
            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(|| {
                "array length is not a literal (unsupported by the layout fn)".to_string()
            })?;
            if !crate::sema::bodies::array_len_fits(n) {
                return Err(format!(
                    "array length {n} exceeds the {}-element build limit",
                    crate::sema::bodies::MAX_ARRAY_LEN
                ));
            }
            let n = usize::try_from(n).map_err(|_| "array length out of range".to_string())?;
            size_of(elem, ctx)?
                .checked_mul(n)
                .ok_or_else(|| "array size overflows usize".to_string())
        }
        Type::Tuple(elems) => {
            let mut total = 0usize;
            for e in elems {
                total = total
                    .checked_add(size_of(e, ctx)?)
                    .ok_or_else(|| "tuple size overflows usize".to_string())?;
            }
            Ok(total)
        }
        Type::Option(inner)
            if matches!(
                &**inner,
                Type::Named(name, _) if name == "GroupId"
            ) =>
        {
            Ok(SLOT)
        }
        Type::Option(inner) => Ok(SLOT + size_of(inner, ctx)?),
        Type::Result(ok, err) => Ok(SLOT + size_of(ok, ctx)?.max(size_of(err, ctx)?)),
        Type::Own(_, _) => Ok(SLOT),
        Type::Static(inner) => size_of(inner, ctx),
        Type::Bytes(Some(len_expr)) => {
            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(|| {
                "Bytes length is not a literal (unsupported by the layout fn)".to_string()
            })?;
            if !crate::sema::bodies::array_len_fits(n) {
                return Err(format!(
                    "`Bytes[N]` length {n} exceeds the {}-element build limit",
                    crate::sema::bodies::MAX_ARRAY_LEN
                ));
            }
            let n = usize::try_from(n).map_err(|_| "Bytes length out of range".to_string())?;
            SLOT.checked_mul(n)
                .ok_or_else(|| "Bytes size overflows usize".to_string())
        }
        Type::Bytes(None) => Ok(SLOT * 2),
        Type::String(len_expr) => {
            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(|| {
                "String capacity is not a literal (unsupported by the layout fn)".to_string()
            })?;
            if !crate::sema::bodies::string_capacity_fits(n) {
                return Err("String capacity out of range".to_string());
            }
            let n = usize::try_from(n).map_err(|_| "String capacity out of range".to_string())?;
            Ok(SLOT
                .checked_mul(1 + n)
                .ok_or_else(|| "String capacity out of range".to_string())?)
        }
        Type::Fn(_, _) => Err("sizing a `fn` value type is not implemented yet".to_string()),
        Type::Generic(_) => {
            Err("sizing a bare generic parameter is not implemented yet".to_string())
        }
        Type::Str => Err("sizing a bare `Str` (unbounded) has no static size".to_string()),
        Type::Named(name, _targs)
            if matches!(
                name.as_str(),
                "Actor"
                    | "Group"
                    | "Instant"
                    | "Duration"
                    | "Admission"
                    | "Peer"
                    | "InterruptCell"
                    | "TurnId"
                    | "CoreId"
                    | "GroupId"
            ) || crate::sema::classes::name_holds_authority(name) =>
        {
            Ok(SLOT)
        }
        Type::Named(name, targs) if name == "Untrusted" => {
            let Some(crate::sema::types::TypeArg::Type(inner)) = targs.first() else {
                return Err("`Untrusted` with no payload type argument".to_string());
            };
            size_of(inner, ctx)
        }
        Type::Named(name, targs) if name == "IoCompletion" => {
            let mut total = 0usize;
            for (_, ty) in io_completion_fields(targs)? {
                total += size_of(&ty, ctx)?;
            }
            Ok(total)
        }
        Type::Named(name, targs) if name == "CallError" => {
            let Some(crate::sema::types::TypeArg::Type(e_ty)) = targs.first() else {
                return Err("`CallError` with no error type argument".to_string());
            };
            let args_ty = crate::sema::bodies::not_admitted_args_type(targs);
            let not_admitted = SLOT + size_of(&args_ty, ctx)?;
            Ok(SLOT + size_of(e_ty, ctx)?.max(not_admitted))
        }
        Type::Named(name, targs)
            if targs.is_empty()
                && matches!(
                    name.as_str(),
                    "BootError"
                        | "Target"
                        | "Failure"
                        | "IoError"
                        | "CompletionOutcome"
                        | "Admission"
                        | "CapacityError"
                ) =>
        {
            Ok(SLOT)
        }
        Type::Named(name, targs) => {
            let key = if targs.is_empty() {
                name.clone()
            } else {
                crate::sema::types::render_type(&Type::Named(name.clone(), targs.clone()))
            };
            if let Some(fields) = ctx.structs.get(&key) {
                let mut total = 0;
                for f in fields {
                    total += size_of(f, ctx)?;
                }
                return Ok(total);
            }
            if let Some(variants) = ctx.enums.get(&key) {
                let mut widest = 0usize;
                for payload in variants {
                    let mut total = 0;
                    for f in payload {
                        total += size_of(f, ctx)?;
                    }
                    widest = widest.max(total);
                }
                return Ok(SLOT + widest);
            }
            if !targs.is_empty() {
                return Err(format!(
                    "sizing an instantiated generic struct/enum `{key}` is not in this layout \
                     context (no matching TypedProgram instantiation)"
                ));
            }
            Err(format!(
                "unknown struct/enum `{name}` in this layout context"
            ))
        }
    }
}

fn strip_static(ty: &Type) -> &Type {
    match ty {
        Type::Static(inner) => strip_static(inner),
        other => other,
    }
}

pub fn is_slotmap_type_name(name: &str) -> bool {
    name == "SlotMap" || name.starts_with("SlotMap[")
}

pub fn is_slotmap_type(ty: &Type) -> bool {
    matches!(strip_static(ty), Type::Named(n, _) if is_slotmap_type_name(n))
}

pub fn field_offset(
    base_ty: &Type,
    index: usize,
    ctx: &LayoutCtx,
) -> Result<(usize, usize), String> {
    match strip_static(base_ty) {
        Type::Tuple(elems) => {
            let mut off = 0usize;
            for e in &elems[..index] {
                off += size_of(e, ctx)?;
            }
            let sz = size_of(&elems[index], ctx)?;
            Ok((off, sz))
        }
        Type::Array(elem, _) => {
            let sz = size_of(elem, ctx)?;
            Ok((sz * index, sz))
        }
        Type::String(n_expr) => {
            let n = crate::sema::bodies::literal_array_len(n_expr).ok_or_else(|| {
                "a `String[..N]` capacity that is not a literal is not supported".to_string()
            })?;
            let n = usize::try_from(n).map_err(|_| "String capacity out of range".to_string())?;
            if index > n {
                return Err(format!(
                    "`String[..{n}]` project index {index} out of range"
                ));
            }
            Ok((8 * index, 8))
        }
        Type::Bytes(None) => {
            if index > 1 {
                return Err(format!("`Bytes` project index {index} out of range"));
            }
            Ok((8 * index, 8))
        }
        Type::Bytes(Some(n_expr)) => {
            let n = crate::sema::bodies::literal_array_len(n_expr).ok_or_else(|| {
                "a `Bytes[N]` length that is not a literal is not supported".to_string()
            })?;
            let n = usize::try_from(n).map_err(|_| "Bytes length out of range".to_string())?;
            if index >= n {
                return Err(format!("`Bytes[{n}]` project index {index} out of range"));
            }
            Ok((8 * index, 8))
        }
        Type::Named(name, targs) => {
            if name == "IoCompletion" {
                let fields = io_completion_fields(targs)?;
                if index >= fields.len() {
                    return Err(format!("`IoCompletion` field index {index} out of range"));
                }
                let mut off = 0usize;
                for (_, ty) in &fields[..index] {
                    off += size_of(ty, ctx)?;
                }
                let sz = size_of(&fields[index].1, ctx)?;
                return Ok((off, sz));
            }
            let key = if targs.is_empty() {
                name.clone()
            } else {
                crate::sema::types::render_type(&Type::Named(name.clone(), targs.to_vec()))
            };
            let fields = ctx
                .structs
                .get(&key)
                .ok_or_else(|| format!("unknown struct `{key}`"))?;
            if index >= fields.len() {
                return Err(format!(
                    "`{key}` field index {index} out of range for {} field(s)",
                    fields.len()
                ));
            }
            let mut off = 0usize;
            for f in &fields[..index] {
                off += size_of(f, ctx)?;
            }
            let sz = size_of(&fields[index], ctx)?;
            Ok((off, sz))
        }
        other => Err(format!(
            "`Project`/`SetField` base is not an aggregate type: {other:?}"
        )),
    }
}

pub fn enum_payload_offset(base_ty: &Type, index: usize, ctx: &LayoutCtx) -> Result<usize, String> {
    const TAG: usize = 8;
    let variants: Vec<Vec<Type>> = match strip_static(base_ty) {
        Type::Option(inner)
            if matches!(
                strip_static(inner),
                Type::Named(name, _) if name == "GroupId"
            ) =>
        {
            return Ok(0);
        }
        Type::Option(inner) => vec![Vec::new(), vec![(**inner).clone()]],
        Type::Result(ok, err) => vec![vec![(**ok).clone()], vec![(**err).clone()]],
        Type::Named(name, targs) if name == "CallError" => {
            let Some(crate::sema::types::TypeArg::Type(e_ty)) = targs.first() else {
                return Err("`CallError` with no error type argument".to_string());
            };
            let args_ty = crate::sema::bodies::not_admitted_args_type(targs);
            vec![
                vec![e_ty.clone()],
                Vec::new(),
                Vec::new(),
                vec![Type::Named("Admission".to_string(), Vec::new()), args_ty],
            ]
        }
        Type::Named(name, targs) => {
            if !targs.is_empty() {
                return Err(
                    "payload access on an instantiated generic enum is not implemented".to_string(),
                );
            }
            ctx.enums
                .get(name)
                .ok_or_else(|| format!("unknown enum `{name}`"))?
                .clone()
        }
        other => {
            return Err(format!("`EnumPayload` base is not an enum type: {other:?}"));
        }
    };
    let mut off = TAG;
    for j in 0..index {
        let mut widest = 0usize;
        for v in &variants {
            if let Some(ty) = v.get(j) {
                let sz = size_of(ty, ctx)?;
                widest = widest.max(sz);
            }
        }
        off += widest;
    }
    Ok(off)
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn scalars_are_one_eight_byte_slot() {
        let ctx = LayoutCtx::default();
        assert_eq!(size_of(&Type::U8, &ctx), Ok(8));
        assert_eq!(size_of(&Type::I64, &ctx), Ok(8));
        assert_eq!(size_of(&Type::Bool, &ctx), Ok(8));
        assert_eq!(size_of(&Type::Unit, &ctx), Ok(8));
    }

    #[test]
    fn untrusted_is_sized_as_its_payload() {
        use crate::sema::types::TypeArg;
        let ctx = LayoutCtx::default();
        let u = Type::Named("Untrusted".to_string(), vec![TypeArg::Type(Type::Usize)]);
        assert_eq!(size_of(&u, &ctx), Ok(8));
        let u32_u = Type::Named("Untrusted".to_string(), vec![TypeArg::Type(Type::U32)]);
        assert_eq!(size_of(&u32_u, &ctx), size_of(&Type::U32, &ctx));
    }

    #[test]
    fn array_size_is_element_stride_times_len() {
        let ctx = LayoutCtx::default();
        let arr = Type::Array(
            Box::new(Type::U8),
            Box::new(crate::syntax::ast::Expr::Int(
                crate::syntax::ast::Span::default(),
                "5".to_string(),
            )),
        );
        assert_eq!(size_of(&arr, &ctx), Ok(8 * 5));
    }

    #[test]
    fn string_bound_size_is_length_word_plus_n_byte_slots() {
        let ctx = LayoutCtx::default();
        let s = Type::String(Box::new(crate::syntax::ast::Expr::Int(
            crate::syntax::ast::Span::default(),
            "8".to_string(),
        )));
        assert_eq!(size_of(&s, &ctx), Ok(8 * (1 + 8)));
    }

    #[test]
    fn tuple_size_is_the_sum_of_components() {
        let ctx = LayoutCtx::default();
        let t = Type::Tuple(vec![Type::U8, Type::U64, Type::Bool]);
        assert_eq!(size_of(&t, &ctx), Ok(8 * 3));
    }

    #[test]
    fn struct_size_is_the_sum_of_field_sizes_in_declaration_order() {
        let mut ctx = LayoutCtx::default();
        ctx.structs.insert(
            "Point".to_string(),
            vec![
                Type::U64,
                Type::U64,
                Type::Array(Box::new(Type::U8), Box::new(dummy_int(2))),
            ],
        );
        let t = Type::Named("Point".to_string(), vec![]);
        assert_eq!(size_of(&t, &ctx), Ok(8 + 8 + 16));
    }

    #[test]
    fn enum_size_is_tag_plus_the_widest_variant() {
        let mut ctx = LayoutCtx::default();
        ctx.enums.insert(
            "Shape".to_string(),
            vec![vec![Type::U64], vec![Type::U64, Type::U64]],
        );
        let t = Type::Named("Shape".to_string(), vec![]);
        assert_eq!(size_of(&t, &ctx), Ok(8 + 16));
    }

    #[test]
    fn option_size_is_tag_plus_the_inner_type() {
        let ctx = LayoutCtx::default();
        let t = Type::Option(Box::new(Type::U64));
        assert_eq!(size_of(&t, &ctx), Ok(8 + 8));
    }

    #[test]
    fn option_group_id_is_one_word_with_none_niche() {
        let ctx = LayoutCtx::default();
        let gid = Type::Named("GroupId".to_string(), vec![]);
        assert_eq!(size_of(&gid, &ctx), Ok(8), "GroupId itself is one word");
        let opt = Type::Option(Box::new(gid));
        assert_eq!(
            size_of(&opt, &ctx),
            Ok(8),
            "Option[GroupId] must stay one bare word (None niche at 0)"
        );
        assert!(
            !crate::codegen::is_aggregate(&opt),
            "Option[GroupId] is by-value, not a by-pointer aggregate"
        );
        assert_eq!(size_of(&Type::Option(Box::new(Type::U32)), &ctx), Ok(16));
        assert!(crate::codegen::is_aggregate(&Type::Option(Box::new(
            Type::U32
        ))));
    }

    #[test]
    fn bare_bytes_handle_is_two_words() {
        let ctx = LayoutCtx::default();
        assert_eq!(size_of(&Type::Bytes(None), &ctx), Ok(16));
    }

    #[test]
    fn instantiated_generic_struct_fails_closed() {
        let ctx = LayoutCtx::default();
        let t = Type::Named(
            "struct:Box[u64]".to_string(),
            vec![crate::sema::types::TypeArg::Type(Type::U64)],
        );
        assert!(size_of(&t, &ctx).is_err());
    }

    fn dummy_int(n: i128) -> crate::syntax::ast::Expr {
        crate::syntax::ast::Expr::Int(crate::syntax::ast::Span::default(), n.to_string())
    }
}

pub fn dump(program: &MwirProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (key, f) in &program.fns {
        let mut header = format!(
            "Fn key={key} ret={} temps={}",
            types::render_type(&f.ret),
            f.temp_count()
        );
        if let Some((t, mode)) = &f.receiver {
            let _ = write!(header, " receiver={t}:{}", mode.as_str());
        }
        if !f.params.is_empty() {
            let ps: Vec<String> = f
                .params
                .iter()
                .map(|(t, mode)| {
                    if *mode == crate::syntax::ast::AccessMode::Read {
                        t.to_string()
                    } else {
                        format!("{t}:{}", mode.as_str())
                    }
                })
                .collect();
            let _ = write!(header, " params=[{}]", ps.join(","));
        }
        push_line(&mut out, 1, &header);
        for (i, ty) in f.temp_types.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("Temp t{i} ty={}", types::render_type(ty)),
            );
        }
        push_line(&mut out, 2, "Body");
        for (i, inst) in f.body.iter().enumerate() {
            let line = format!("{i:04}: {}", fmt_inst(inst));
            push_line(&mut out, 3, &line);
        }
    }
    if !program.rodata.is_empty() {
        push_line(&mut out, 1, "Rodata");
        for (i, bytes) in program.rodata.iter().enumerate() {
            push_line(&mut out, 2, &format!("{i}: {}", render_bytes(bytes)));
        }
    }
    out
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn render_bytes(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

pub(crate) fn fmt_inst(inst: &Inst) -> String {
    match inst {
        Inst::ConstInt { dst, ty, value } => {
            format!(
                "ConstInt dst={dst} ty={} value={value}",
                types::render_type(ty)
            )
        }
        Inst::ConstBool { dst, value } => format!("ConstBool dst={dst} value={value}"),
        Inst::ConstFloat { dst, ty, bits } => {
            format!(
                "ConstFloat dst={dst} ty={} bits={bits}",
                types::render_type(ty)
            )
        }
        Inst::ConstChar { dst, value } => format!("ConstChar dst={dst} value={value:?}"),
        Inst::ConstUnit { dst } => format!("ConstUnit dst={dst}"),
        Inst::ConstText { dst, data } => format!("ConstText dst={dst} data={data}"),
        Inst::Copy { dst, src } => format!("Copy dst={dst} src={src}"),
        Inst::MmioRead {
            dst,
            base,
            offset,
            ty,
        } => format!(
            "MmioRead dst={dst} base={base} offset={offset:#x} ty={}",
            types::render_type(ty)
        ),
        Inst::MmioWrite {
            base,
            offset,
            ty,
            value,
        } => format!(
            "MmioWrite base={base} offset={offset:#x} ty={} value={value}",
            types::render_type(ty)
        ),
        Inst::PlacedIndexGet {
            dst,
            base,
            field_offset,
            index,
            len,
            elem_stride,
            ty,
        }
        | Inst::PlacedIndexGetProven {
            dst,
            base,
            field_offset,
            index,
            len,
            elem_stride,
            ty,
        } => format!(
            "{} dst={dst} base={base} field_offset={field_offset:#x} \
             index={index} len={len} elem_stride={elem_stride} ty={}",
            if matches!(inst, Inst::PlacedIndexGetProven { .. }) {
                "PlacedIndexGetProven"
            } else {
                "PlacedIndexGet"
            },
            types::render_type(ty)
        ),
        Inst::PlacedIndexSet {
            base,
            field_offset,
            index,
            value,
            len,
            elem_stride,
            ty,
        }
        | Inst::PlacedIndexSetProven {
            base,
            field_offset,
            index,
            value,
            len,
            elem_stride,
            ty,
        } => format!(
            "{} base={base} field_offset={field_offset:#x} index={index} \
             value={value} len={len} elem_stride={elem_stride} ty={}",
            if matches!(inst, Inst::PlacedIndexSetProven { .. }) {
                "PlacedIndexSetProven"
            } else {
                "PlacedIndexSet"
            },
            types::render_type(ty)
        ),
        Inst::BytesIndexGet { dst, base, index } => {
            format!("BytesIndexGet dst={dst} base={base} index={index}")
        }
        Inst::MemLoad {
            dst,
            base,
            offset,
            width,
        } => format!("MemLoad dst={dst} base={base} offset={offset:#x} width={width}"),
        Inst::MemStore {
            base,
            offset,
            value,
            width,
        } => format!("MemStore base={base} offset={offset:#x} value={value} width={width}"),
        Inst::PtrOffset { dst, base, offset } => {
            format!("PtrOffset dst={dst} base={base} offset={offset:#x}")
        }
        Inst::TurnAddrFromId { dst, id } => {
            format!("TurnAddrFromId dst={dst} id={id}")
        }
        Inst::Abort { message } => format!("Abort message={message:?}"),
        Inst::LoadIrqVector { dst, driver } => {
            format!("LoadIrqVector dst={dst} driver={driver}")
        }
        Inst::InterruptCellLoadAcquire {
            dst,
            field_off,
            width,
        } => format!("InterruptCellLoadAcquire dst={dst} field_off={field_off} width={width}"),
        Inst::InterruptCellStoreRelease {
            field_off,
            width,
            value,
        } => format!("InterruptCellStoreRelease field_off={field_off} width={width} value={value}"),
        Inst::InterruptCellSwapAcquire {
            dst,
            field_off,
            width,
            value,
        } => format!(
            "InterruptCellSwapAcquire dst={dst} field_off={field_off} width={width} value={value}"
        ),
        Inst::InterruptCellFetchOrRelease {
            dst,
            field_off,
            width,
            value,
        } => format!(
            "InterruptCellFetchOrRelease dst={dst} field_off={field_off} width={width} value={value}"
        ),
        Inst::Dmb { option } => format!("Dmb option={option}"),
        Inst::Wake { driver } => format!("Wake driver={driver}"),
        Inst::Now { dst } => format!("Now dst={dst}"),
        Inst::Entropy { dst, n } => format!("Entropy dst={dst} n={n}"),
        Inst::SlotMapMint { map } => format!("SlotMapMint map={map}"),
        Inst::MakeAggregate { dst, elems } => {
            format!("MakeAggregate dst={dst} elems=[{}]", join_temps(elems))
        }
        Inst::FormatScalar {
            dst,
            src,
            src_ty,
            capacity,
        } => {
            format!(
                "FormatScalar dst={dst} src={src} src_ty={} capacity={capacity}",
                crate::sema::types::render_type(src_ty)
            )
        }
        Inst::StringConcat {
            dst,
            lhs,
            rhs,
            lhs_cap,
            rhs_cap,
        } => {
            format!(
                "StringConcat dst={dst} lhs={lhs} rhs={rhs} lhs_cap={lhs_cap} rhs_cap={rhs_cap}"
            )
        }
        Inst::Project { dst, base, index } => {
            format!("Project dst={dst} base={base} index={index}")
        }
        Inst::SetField { base, index, value } => {
            format!("SetField base={base} index={index} value={value}")
        }
        Inst::IndexGet {
            dst,
            base,
            index,
            len,
        }
        | Inst::IndexGetProven {
            dst,
            base,
            index,
            len,
        } => {
            format!(
                "{} dst={dst} base={base} index={index} len={len}",
                if matches!(inst, Inst::IndexGetProven { .. }) {
                    "IndexGetProven"
                } else {
                    "IndexGet"
                }
            )
        }
        Inst::IndexSet {
            base,
            index,
            value,
            len,
        }
        | Inst::IndexSetProven {
            base,
            index,
            value,
            len,
        } => {
            format!(
                "{} base={base} index={index} value={value} len={len}",
                if matches!(inst, Inst::IndexSetProven { .. }) {
                    "IndexSetProven"
                } else {
                    "IndexSet"
                }
            )
        }
        Inst::MakeEnum { dst, tag, payload } => {
            format!(
                "MakeEnum dst={dst} tag={tag} payload=[{}]",
                join_temps(payload)
            )
        }
        Inst::EnumTag { dst, src } => format!("EnumTag dst={dst} src={src}"),
        Inst::EnumPayload { dst, src, index } => {
            format!("EnumPayload dst={dst} src={src} index={index}")
        }
        Inst::ArithChecked {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort,
        } => format!(
            "ArithChecked op={} ty={} dst={dst} lhs={lhs} rhs={rhs} abort={abort:?}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::ArithWrapping {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => format!(
            "ArithWrapping op={} ty={} dst={dst} lhs={lhs} rhs={rhs}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::DivRem {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort_zero,
            abort_overflow,
        } => format!(
            "DivRem op={} ty={} dst={dst} lhs={lhs} rhs={rhs} abort_zero={abort_zero:?} abort_overflow={abort_overflow:?}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::Shift {
            dst,
            op,
            ty,
            lhs,
            rhs,
            bits,
            lost,
        } => {
            let mut s = format!(
                "Shift op={} ty={} dst={dst} lhs={lhs} rhs={rhs} bits={bits}",
                op.as_str(),
                types::render_type(ty)
            );
            if let Some(l) = lost {
                let _ = write!(s, " lost={l:?}");
            }
            s
        }
        Inst::Bitwise {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => format!(
            "Bitwise op={} ty={} dst={dst} lhs={lhs} rhs={rhs}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::Compare {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => format!(
            "Compare op={} ty={} dst={dst} lhs={lhs} rhs={rhs}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::Convert {
            dst,
            ty,
            src,
            abort,
        } => format!(
            "Convert ty={} dst={dst} src={src} abort={abort:?}",
            types::render_type(ty)
        ),
        Inst::Neg {
            dst,
            ty,
            src,
            abort,
        } => format!(
            "Neg ty={} dst={dst} src={src} abort={abort:?}",
            types::render_type(ty)
        ),
        Inst::BitNot { dst, ty, src } => {
            format!("BitNot ty={} dst={dst} src={src}", types::render_type(ty))
        }
        Inst::Not { dst, src } => format!("Not dst={dst} src={src}"),
        Inst::BoolAnd { dst, lhs, rhs } => format!("BoolAnd dst={dst} lhs={lhs} rhs={rhs}"),
        Inst::Jump { target } => format!("Jump target={target:04}"),
        Inst::JumpIfFalse { cond, target } => {
            format!("JumpIfFalse cond={cond} target={target:04}")
        }
        Inst::Call {
            dst,
            write_backs,
            key,
            args,
        } => {
            let mut s = format!("Call key={key} dst={dst} args=[{}]", join_temps(args));
            if !write_backs.is_empty() {
                let parts: Vec<String> = write_backs
                    .iter()
                    .map(|(i, t)| format!("{i}:{t}"))
                    .collect();
                let _ = write!(s, " write_backs=[{}]", parts.join(","));
            }
            s
        }
        Inst::Return { value } => match value {
            Some(v) => format!("Return value={v}"),
            None => "Return".to_string(),
        },
        Inst::AssertFail { message } => match message {
            Some(m) => format!("AssertFail message={m:?}"),
            None => "AssertFail".to_string(),
        },
    }
}

fn join_temps(ts: &[Temp]) -> String {
    ts.iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",")
}
