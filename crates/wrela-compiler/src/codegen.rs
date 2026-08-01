use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use crate::cost::{CostRule, EmittedWord, FlagEffect, MEM_SP_REG, MemClass, MemRef};
use crate::encode::{self, Cond};
use crate::mwir::{self, Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::regalloc;
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

thread_local! {
    static OMIT_DMB: Cell<bool> = const { Cell::new(false) };
}

pub fn set_omit_dmb(omit: bool) {
    OMIT_DMB.with(|c| c.set(omit));
}

fn omit_dmb() -> bool {
    OMIT_DMB.with(|c| c.get())
}

thread_local! {
    static BLOCK_COUNT: Cell<bool> = const { Cell::new(false) };
    static NEXT_BLOCK_ID: Cell<u32> = const { Cell::new(0) };
    static BLOCK_BRIDGE: Cell<bool> = const { Cell::new(false) };
    static BLOCK_SPANS: RefCell<Vec<BlockSpan>> = const { RefCell::new(Vec::new()) };
}

pub fn set_block_count(enabled: bool) {
    BLOCK_COUNT.with(|c| c.set(enabled));
    NEXT_BLOCK_ID.with(|c| c.set(0));
}

fn block_count() -> bool {
    BLOCK_COUNT.with(|c| c.get())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSpan {
    pub fn_key: String,
    pub block_index: u32,
    pub id: u32,
    pub word_start: usize,
    pub word_end: usize,
}

pub fn set_block_bridge(enabled: bool) {
    BLOCK_BRIDGE.with(|c| c.set(enabled));
    if enabled {
        BLOCK_SPANS.with(|s| s.borrow_mut().clear());
        NEXT_BLOCK_ID.with(|c| c.set(0));
    }
}

fn block_bridge() -> bool {
    BLOCK_BRIDGE.with(|c| c.get())
}

pub fn block_spans() -> Vec<BlockSpan> {
    BLOCK_SPANS.with(|s| s.borrow().clone())
}

fn record_block_span(fn_key: &str, block_index: u32, id: u32, word_start: usize, word_end: usize) {
    BLOCK_SPANS.with(|s| {
        s.borrow_mut().push(BlockSpan {
            fn_key: fn_key.to_string(),
            block_index,
            id,
            word_start,
            word_end,
        })
    });
}

fn block_ids_active() -> bool {
    block_count() || block_bridge()
}

fn record_spans(fn_key: &str, block_ids: &[Option<u32>], word_offsets: &[usize], code_len: usize) {
    let leaders: Vec<(usize, u32)> = block_ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| id.map(|id| (i, id)))
        .collect();
    for (n, &(mwir_idx, id)) in leaders.iter().enumerate() {
        let word_start = if n == 0 { 0 } else { word_offsets[mwir_idx] };
        let word_end = match leaders.get(n + 1) {
            Some(&(next_idx, _)) => word_offsets[next_idx],
            None => code_len,
        };
        record_block_span(fn_key, n as u32, id, word_start, word_end);
    }
}

pub fn block_count_enabled() -> bool {
    block_count()
}

const BLOCK_HIT_KEY: &str = "__wrela_block_hit";

fn block_count_instruments(key: &str) -> bool {
    block_ids_active() && key != BLOCK_HIT_KEY
}

pub fn block_ids_assigned() -> u32 {
    NEXT_BLOCK_ID.with(|c| c.get())
}

thread_local! {
    static NARROW_IMM: Cell<bool> = const { Cell::new(false) };
}

pub fn set_narrow_imm(enabled: bool) {
    NARROW_IMM.with(|c| c.set(enabled));
}

pub(crate) fn narrow_imm() -> bool {
    NARROW_IMM.with(|c| c.get())
}

thread_local! {
    static ADR_ADDRESSING: Cell<bool> = const { Cell::new(false) };
}

pub fn set_adr_addressing(enabled: bool) {
    ADR_ADDRESSING.with(|c| c.set(enabled));
}

pub(crate) fn adr_addressing() -> bool {
    ADR_ADDRESSING.with(|c| c.get())
}

thread_local! {
    static BFX_NARROW: Cell<bool> = const { Cell::new(false) };
    static MASK_CHECK: Cell<bool> = const { Cell::new(false) };
    static WIDE_IMM_FORMS: Cell<bool> = const { Cell::new(false) };
}

pub fn set_bfx_narrow(enabled: bool) {
    BFX_NARROW.with(|c| c.set(enabled));
}

pub(crate) fn bfx_narrow() -> bool {
    BFX_NARROW.with(|c| c.get())
}

pub fn set_mask_check(enabled: bool) {
    MASK_CHECK.with(|c| c.set(enabled));
}

pub(crate) fn mask_check() -> bool {
    MASK_CHECK.with(|c| c.get())
}

pub fn set_wide_imm_forms(enabled: bool) {
    WIDE_IMM_FORMS.with(|c| c.set(enabled));
}

pub(crate) fn wide_imm_forms() -> bool {
    WIDE_IMM_FORMS.with(|c| c.get())
}

thread_local! {
    static FRAMELESS_FNS: Cell<bool> = const { Cell::new(false) };
    static TAIL_CALLS: Cell<bool> = const { Cell::new(false) };
}

pub fn set_tail_calls(enabled: bool) {
    TAIL_CALLS.with(|c| c.set(enabled));
}

pub(crate) fn tail_calls() -> bool {
    TAIL_CALLS.with(|c| c.get())
}

pub fn set_frameless_fns(enabled: bool) {
    FRAMELESS_FNS.with(|c| c.set(enabled));
}

pub(crate) fn frameless_fns() -> bool {
    FRAMELESS_FNS.with(|c| c.get())
}

thread_local! {
    static BRANCH_CLEANUP: Cell<bool> = const { Cell::new(false) };
}

pub fn set_branch_cleanup(enabled: bool) {
    BRANCH_CLEANUP.with(|c| c.set(enabled));
}

pub(crate) fn branch_cleanup() -> bool {
    BRANCH_CLEANUP.with(|c| c.get())
}

fn plan_branch_elision(
    n: usize,
    leaders: &[bool],
    target_of: impl Fn(usize) -> Option<usize>,
) -> Vec<bool> {
    let mut elide = vec![false; n];
    if !branch_cleanup() {
        return elide;
    }
    for (i, e) in elide.iter_mut().enumerate() {
        let Some(t) = target_of(i) else { continue };
        if t != i + 1 {
            continue;
        }
        if i + 1 < n && leaders[i + 1] {
            continue;
        }
        *e = true;
    }
    elide
}

fn sync_branch_elision(body: &[Inst]) -> Vec<bool> {
    let n = body.len();
    let leaders = mwir_block_leaders(body);
    plan_branch_elision(n, &leaders, |i| match &body[i] {
        Inst::Jump { target } => Some(*target),
        Inst::Return { .. } => Some(n),
        _ => None,
    })
}

const X_LR: u8 = 30;
const X_SP: u8 = 31;
const X_ZR: u8 = 31;

const X_A: u8 = 9;
const X_B: u8 = 10;
const X_C: u8 = 11;
const X_D: u8 = 12;
const X_E: u8 = 13;
const X_F: u8 = 14;
const X_FRAME: u8 = 28;

fn reg_name(r: u8) -> String {
    match r {
        X_SP => "sp".to_string(),
        X_LR => "lr".to_string(),
        _ => format!("x{r}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    pub message: String,
}

impl CodegenError {
    fn unimplemented(what: &str) -> CodegenError {
        CodegenError {
            message: format!("codegen for {what} is not implemented yet"),
        }
    }

    fn internal(msg: impl Into<String>) -> CodegenError {
        CodegenError {
            message: format!("internal error: {}", msg.into()),
        }
    }
}

pub const FAIL_CLOSED_PREFIX: &str = "fail-closed: ";

fn alloc_block_id() -> Result<u32, CodegenError> {
    let id = NEXT_BLOCK_ID.with(|c| {
        let id = c.get();
        c.set(id.saturating_add(1));
        id
    });
    if block_count() && id as usize >= crate::rtconfig::BLOCK_POOL_COUNT {
        return Err(CodegenError {
            message: format!(
                "{FAIL_CLOSED_PREFIX}block-count pool exhausted (BLOCK_POOL_COUNT={})",
                crate::rtconfig::BLOCK_POOL_COUNT
            ),
        });
    }
    Ok(id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reloc {
    Call {
        word: usize,
        key: String,
    },
    Rodata {
        word_adrp: usize,
        byte_offset: usize,
    },
    RodataAdr {
        word: usize,
        byte_offset: usize,
    },
    AbortFixed {
        word: usize,
    },
    AbortVal {
        word: usize,
    },
    CheckpointService {
        word: usize,
    },
    TurnFrameAddr {
        word: usize,
        key: String,
    },
    TurnIdImm {
        word: usize,
        key: String,
    },
    TurnsBase {
        word: usize,
    },
    TurnStride {
        word: usize,
    },
    GroupArenaBase {
        word: usize,
    },
    IrqVector {
        word: usize,
        driver: String,
    },
    WakePending {
        word: usize,
        driver: String,
    },
    MailboxAddr {
        word: usize,
        actor: String,
        field: MailboxField,
    },
    RrCursor {
        word: usize,
        core: usize,
    },
    RingAddr {
        word: usize,
        ring_index: usize,
        field: RingField,
    },
    DriverState {
        word: usize,
        driver: String,
    },
    DeviceRegsBase {
        word: usize,
        device: usize,
    },
    PoolBase {
        word: usize,
        pool: String,
    },
    PoolSlot {
        word: usize,
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxField {
    Ring,
    Head,
    Tail,
    Count,
    State,
    Turn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingField {
    Ring,
    Head,
    Tail,
    Count,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodegenFn {
    pub frame_size: usize,
    pub code: Vec<EmittedWord>,
    pub relocs: Vec<Reloc>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodegenProgram {
    pub fns: BTreeMap<String, CodegenFn>,
    pub rodata: Vec<Vec<u8>>,
    pub conventions: BTreeMap<String, regalloc::Convention>,
}

struct RodataPool {
    entries: Vec<Vec<u8>>,
    index: BTreeMap<Vec<u8>, usize>,
}

impl RodataPool {
    fn new() -> RodataPool {
        RodataPool {
            entries: Vec::new(),
            index: BTreeMap::new(),
        }
    }

    fn seed(&mut self, initial: &[Vec<u8>]) {
        for bytes in initial {
            self.intern(bytes.clone());
        }
    }

    fn intern(&mut self, bytes: Vec<u8>) -> usize {
        if let Some(&i) = self.index.get(&bytes) {
            return i;
        }
        let i = self.entries.len();
        self.index.insert(bytes.clone(), i);
        self.entries.push(bytes);
        i
    }

    fn byte_offset(&self, idx: usize) -> usize {
        self.entries[..idx].iter().map(Vec::len).sum()
    }
}

fn strip_wrappers(ty: &Type) -> &Type {
    match ty {
        Type::Static(inner) => strip_wrappers(inner),
        other => other,
    }
}

fn unwrap_own_ref(ty: &Type) -> &Type {
    match ty {
        Type::Own(_, inner) => inner,
        other => other,
    }
}

pub(crate) fn is_aggregate(ty: &Type) -> bool {
    match strip_wrappers(ty) {
        Type::Own(..) => false,
        Type::Bytes(None) => true,
        Type::Named(name, _)
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
            false
        }
        Type::Option(inner)
            if matches!(
                strip_wrappers(inner),
                Type::Named(name, _) if name == "GroupId"
            ) =>
        {
            false
        }
        Type::Named(..) | Type::Tuple(_) | Type::Array(..) | Type::Option(_) | Type::Result(..) => {
            true
        }
        Type::String(_) => true,
        Type::Bytes(Some(_)) => true,
        _ => false,
    }
}

fn is_option_group_id(ty: &Type) -> bool {
    match strip_wrappers(ty) {
        Type::Option(inner) => {
            matches!(strip_wrappers(inner), Type::Named(name, _) if name == "GroupId")
        }
        _ => false,
    }
}

fn int_shape(ty: &Type) -> Option<(u32, bool)> {
    match strip_wrappers(ty) {
        Type::U8 => Some((8, false)),
        Type::U16 => Some((16, false)),
        Type::U32 => Some((32, false)),
        Type::U64 | Type::Usize => Some((64, false)),
        Type::I8 => Some((8, true)),
        Type::I16 => Some((16, true)),
        Type::I32 => Some((32, true)),
        Type::I64 | Type::Isize => Some((64, true)),
        _ => None,
    }
}

fn int_bounds_i64(ty: &Type) -> Option<(i64, i64)> {
    match strip_wrappers(ty) {
        Type::U8 => Some((0, u8::MAX as i64)),
        Type::U16 => Some((0, u16::MAX as i64)),
        Type::U32 => Some((0, u32::MAX as i64)),
        Type::U64 | Type::Usize => Some((0, i64::MAX)),
        Type::I8 => Some((i8::MIN as i64, i8::MAX as i64)),
        Type::I16 => Some((i16::MIN as i64, i16::MAX as i64)),
        Type::I32 => Some((i32::MIN as i64, i32::MAX as i64)),
        Type::I64 | Type::Isize => Some((i64::MIN, i64::MAX)),
        _ => None,
    }
}

fn int_bounds_for(bits: u32, signed: bool) -> (i64, i64) {
    debug_assert!(bits < 64, "int_bounds_for is exact only below 64 bits");
    if signed {
        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
    } else {
        (0, (1i64 << bits) - 1)
    }
}

fn is_float(ty: &Type) -> bool {
    matches!(strip_wrappers(ty), Type::F32 | Type::F64)
}

struct Frame {
    temp_offset: Vec<usize>,
    temp_size: Vec<usize>,
    self_ptr_off: Option<usize>,
    mut_param_ptr_offs: Vec<(Temp, usize)>,
    ret_ptr_off: Option<usize>,
    reply_stage_off: Option<usize>,
    entropy_scratch_off: Option<usize>,
    entropy_scratch_size: usize,
    lr_off: usize,
    lr_saved: bool,
    size: usize,
    frameless: bool,
    virt_to_reg: BTreeMap<usize, u8>,
}

const VIRT_SLOT_BASE: usize = 1 << 20;

pub const FRAME_SP_ALIGN_BYTES: u64 = 16;

pub const FRAME_SLOT_BYTES: u64 = 8;

fn round_up_16(n: usize) -> usize {
    let a = FRAME_SP_ALIGN_BYTES as usize;
    (n + a - 1) & !(a - 1)
}

fn build_frame(
    f: &MwirFn,
    layout: &LayoutCtx,
    reply_stage_size: usize,
    entropy_scratch_size: usize,
    slot_bias: usize,
    assign: &regalloc::Assignment,
    save_lr: bool,
) -> Result<Frame, CodegenError> {
    let mut offset = 0usize;
    let mut temp_offset = Vec::with_capacity(f.temp_types.len());
    let mut temp_size = Vec::with_capacity(f.temp_types.len());
    let mut virt_to_reg: BTreeMap<usize, u8> = BTreeMap::new();
    let mut next_virt = VIRT_SLOT_BASE;
    for (t, ty) in f.temp_types.iter().enumerate() {
        let sz = mwir::size_of(ty, layout).map_err(|e| CodegenError::unimplemented(&e))?;
        match assign.of(t) {
            Some(reg) => {
                temp_offset.push(next_virt);
                virt_to_reg.insert(next_virt, reg);
                next_virt += FRAME_SLOT_BYTES as usize;
                temp_size.push(sz);
            }
            None => {
                temp_offset.push(offset);
                temp_size.push(sz);
                offset += sz;
            }
        }
    }
    let self_ptr_off = if f.receiver.is_some() {
        let o = offset;
        offset += 8;
        Some(o)
    } else {
        None
    };
    let mut mut_param_ptr_offs = Vec::new();
    for (p, mode) in &f.params {
        if *mode == AccessMode::Mut {
            mut_param_ptr_offs.push((*p, offset));
            offset += 8;
        }
    }
    let ret_ptr_off = if is_aggregate(&f.ret) {
        let o = offset;
        offset += 8;
        Some(o)
    } else {
        None
    };
    let reply_stage_off = if reply_stage_size > 0 {
        let o = offset;
        offset += reply_stage_size;
        Some(o)
    } else {
        None
    };
    let (entropy_scratch_off, entropy_scratch_size) = if entropy_scratch_size > 0 {
        let o = offset;
        offset += (entropy_scratch_size + 7) & !7;
        (Some(o), entropy_scratch_size)
    } else {
        (None, 0)
    };
    let elide_changes_size = round_up_16(offset) != round_up_16(offset + 8);
    let lr_saved = (save_lr || slot_bias != 0) || (elide_changes_size && offset != 0);
    let lr_off = offset;
    if lr_saved {
        offset += 8;
    }
    let frameless = offset == 0;
    let size = round_up_16(offset);
    if size + slot_bias > 4095 {
        return Err(CodegenError::unimplemented(&format!(
            "frames larger than {} bytes (the ADD/SUB-immediate imm12 range, less this fn's \
             own {slot_bias}-byte slot bias)",
            4095 - slot_bias
        )));
    }
    Ok(Frame {
        temp_offset,
        temp_size,
        self_ptr_off,
        mut_param_ptr_offs,
        ret_ptr_off,
        reply_stage_off,
        entropy_scratch_off,
        entropy_scratch_size,
        lr_off,
        lr_saved,
        size,
        frameless,
        virt_to_reg,
    })
}

impl Frame {
    fn off(&self, t: Temp) -> usize {
        self.temp_offset[t.0]
    }

    fn size_of_temp(&self, t: Temp) -> usize {
        self.temp_size[t.0]
    }

    fn reg_at(&self, off: usize) -> Option<u8> {
        if off < VIRT_SLOT_BASE {
            None
        } else {
            self.virt_to_reg.get(&off).copied()
        }
    }

    fn home_mask(&self) -> u32 {
        let mut m = 0u32;
        for &r in self.virt_to_reg.values() {
            if r > regalloc::MAX_HINT_REG {
                m |= 1u32 << (r & 31);
            }
        }
        m
    }

    fn is_stray_virtual(&self, off: usize) -> bool {
        off >= VIRT_SLOT_BASE && !self.virt_to_reg.contains_key(&off)
    }

    fn temp_at_offset(&self, off: usize) -> Option<(usize, bool)> {
        for (t, (&base, &size)) in self
            .temp_offset
            .iter()
            .zip(self.temp_size.iter())
            .enumerate()
        {
            if base >= VIRT_SLOT_BASE {
                continue;
            }
            if off >= base && off < base + size.max(1) {
                return Some((t, off == base));
            }
        }
        None
    }
}

fn field_offset_size(
    base_ty: &Type,
    index: usize,
    layout: &LayoutCtx,
) -> Result<(usize, usize), CodegenError> {
    mwir::field_offset(base_ty, index, layout).map_err(|e| {
        if e.contains("not a literal") || e.contains("not implemented") {
            CodegenError::unimplemented(&e)
        } else {
            CodegenError::internal(e)
        }
    })
}

fn enum_payload_offset(
    base_ty: &Type,
    index: usize,
    layout: &LayoutCtx,
) -> Result<usize, CodegenError> {
    mwir::enum_payload_offset(base_ty, index, layout).map_err(|e| {
        if e.contains("not implemented") {
            CodegenError::unimplemented(&e)
        } else {
            CodegenError::internal(e)
        }
    })
}

struct FnCtx<'a> {
    frame: &'a Frame,
    layout: &'a LayoutCtx,
    rodata: &'a mut RodataPool,
    word_offsets: &'a [usize],
    words: Vec<EmittedWord>,
    relocs: Vec<Reloc>,
    slot_base: u8,
    slot_bias: usize,
    cold_seq: u64,
    slot_accesses: Vec<(usize, regalloc::Touch, usize, u8)>,
    resident_misuse: Option<String>,
    home_mask: u32,
    home_def_ok: Option<u8>,
    elide_branch: bool,
}

fn check_push_shape(rule: CostRule, dst: Option<u8>, srcs: &[u8], mem: Option<&MemRef>) {
    if rule == CostRule::Call {
        assert_eq!(
            dst,
            Some(0),
            "Call must declare dst=Some(0) (x0 return/clobber)"
        );
    }
    if rule.is_load() {
        if let Some(m) = mem {
            if !memref_is_unique_cold(m) {
                assert!(
                    !srcs.is_empty(),
                    "{rule:?} with known address MemRef needs ≥1 src (address base)"
                );
            }
        }
    }
    if rule.is_store() {
        assert_eq!(
            dst, None,
            "{rule:?} produces no register; its data register belongs in srcs"
        );
        if let Some(m) = mem {
            if let Some(base) = memref_nonunique_base(m) {
                assert!(
                    srcs.iter().any(|&r| r == base),
                    "{rule:?} with non-unique MemRef requires base reg {base} ∈ srcs (got {srcs:?})"
                );
            }
        }
    }
}

fn memref_is_unique_cold(m: &MemRef) -> bool {
    m.class == MemClass::Cold && (m.key & (1u64 << 63)) != 0
}

fn memref_nonunique_base(m: &MemRef) -> Option<u8> {
    if memref_is_unique_cold(m) {
        None
    } else if m.class == MemClass::Stack {
        Some(MEM_SP_REG)
    } else {
        Some(((m.key >> 48) & 0xFF) as u8)
    }
}

impl<'a> FnCtx<'a> {
    fn push(&mut self, word: u32, text: String, rule: CostRule, dst: Option<u8>, srcs: &[u8]) {
        let mem = if rule.is_load() || rule.is_store() {
            Some(self.alloc_unique_cold())
        } else {
            None
        };
        self.push_mem(word, text, rule, dst, srcs, mem);
    }

    fn add_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_add_reg(d, a, b, true),
            format!("add {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Alu,
            Some(d),
            &[a, b],
        );
    }

    fn mul_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_mul(d, a, b, true),
            format!("mul {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Mul,
            Some(d),
            &[a, b],
        );
    }

    fn orr_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_orr_reg(d, a, b, true),
            format!("orr {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Alu,
            Some(d),
            &[a, b],
        );
    }

    fn and_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_and_reg(d, a, b, true),
            format!("and {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Alu,
            Some(d),
            &[a, b],
        );
    }

    fn cmp_reg(&mut self, a: u8, b: u8) {
        self.push_flags(
            encode::enc_cmp_reg(a, b, true),
            format!("cmp {}, {}", reg_name(a), reg_name(b)),
            CostRule::Alu,
            None,
            &[a, b],
            FlagEffect::Write,
        );
    }

    fn push_flags(
        &mut self,
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<u8>,
        srcs: &[u8],
        flags: FlagEffect,
    ) {
        check_push_shape(rule, dst, srcs, None);
        self.check_home_write(dst, &text);
        let mut ew = EmittedWord::new(word, text, rule, dst, srcs);
        ew.flags = flags;
        self.words.push(ew);
    }

    fn check_home_write(&mut self, dst: Option<u8>, text: &str) {
        let Some(d) = dst else { return };
        if self.home_mask & (1u32 << (d & 31)) == 0 || self.home_def_ok == Some(d) {
            return;
        }
        self.note_alloc_divergence(&format!(
            "`{text}` writes x{d}, which is a register-allocated temp's home, outside that \
             temp's own definition"
        ));
    }

    fn push_mem(
        &mut self,
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<u8>,
        srcs: &[u8],
        mem: Option<MemRef>,
    ) {
        let mem = match mem {
            None if rule.is_load() || rule.is_store() => Some(self.alloc_unique_cold()),
            m => m,
        };
        check_push_shape(rule, dst, srcs, mem.as_ref());
        self.check_home_write(dst, &text);
        let mut ew = EmittedWord::new(word, text, rule, dst, srcs);
        ew.mem = mem;
        self.words.push(ew);
    }

    fn alloc_unique_cold(&mut self) -> MemRef {
        let seq = self.cold_seq;
        self.cold_seq = self.cold_seq.wrapping_add(1);
        MemRef::cold_unique(seq)
    }

    fn cur_word(&self) -> usize {
        self.words.len()
    }

    fn mov_reg(&mut self, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        self.push(
            encode::enc_mov_reg(dst, src, true),
            format!("mov {}, {}", reg_name(dst), reg_name(src)),
            CostRule::Alu,
            Some(dst),
            &[src],
        );
    }

    fn note_resident_misuse(&mut self, what: &str, off: usize) {
        self.note_alloc_divergence(&format!(
            "register-allocated temp reached through {what} at virtual slot offset {off}"
        ));
    }

    fn note_alloc_divergence(&mut self, msg: &str) {
        if self.resident_misuse.is_none() {
            self.resident_misuse = Some(msg.to_string());
        }
    }

    fn use_slot(&mut self, scratch: u8, off: usize) -> u8 {
        if let Some(home) = self.frame.reg_at(off) {
            self.slot_accesses
                .push((off, regalloc::Touch::Read, self.words.len(), scratch));
            return home;
        }
        self.load_slot(scratch, off);
        scratch
    }

    fn def_reg(&mut self, scratch: u8, off: usize) -> u8 {
        match self.frame.reg_at(off) {
            Some(home) => {
                self.home_def_ok = Some(home);
                home
            }
            None => scratch,
        }
    }

    fn load_slot(&mut self, reg: u8, off: usize) {
        self.slot_accesses
            .push((off, regalloc::Touch::Read, self.words.len(), reg));
        if let Some(home) = self.frame.reg_at(off) {
            self.mov_reg(reg, home);
            return;
        }
        if self.frame.is_stray_virtual(off) {
            self.note_resident_misuse("load_slot", off);
            return;
        }
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        let mem = MemRef::for_base_imm(base, off as u64);
        self.push_mem(
            encode::enc_ldr_x_imm(reg, base, off),
            format!("ldr {}, [{}, #{off}]", reg_name(reg), reg_name(base)),
            CostRule::Load,
            Some(reg),
            &[base],
            Some(mem),
        );
    }

    fn store_slot(&mut self, reg: u8, off: usize) {
        self.slot_accesses
            .push((off, regalloc::Touch::Write, self.words.len(), reg));
        if let Some(home) = self.frame.reg_at(off) {
            self.home_def_ok = Some(home);
            self.mov_reg(home, reg);
            self.home_def_ok = None;
            return;
        }
        self.home_def_ok = None;
        if self.frame.is_stray_virtual(off) {
            self.note_resident_misuse("store_slot", off);
            return;
        }
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        let mem = MemRef::for_base_imm(base, off as u64);
        self.push_mem(
            encode::enc_str_x_imm(reg, base, off),
            format!("str {}, [{}, #{off}]", reg_name(reg), reg_name(base)),
            CostRule::Store,
            None,
            &[reg, base],
            Some(mem),
        );
    }

    fn load_ptr(&mut self, reg: u8, base_reg: u8, byte_off: usize) {
        let byte_off = byte_off as u16;
        let mem = MemRef::for_base_imm(base_reg, byte_off as u64);
        self.push_mem(
            encode::enc_ldr_x_imm(reg, base_reg, byte_off),
            format!(
                "ldr {}, [{}, #{byte_off}]",
                reg_name(reg),
                reg_name(base_reg)
            ),
            CostRule::Load,
            Some(reg),
            &[base_reg],
            Some(mem),
        );
    }

    fn store_ptr(&mut self, reg: u8, base_reg: u8, byte_off: usize) {
        let byte_off = byte_off as u16;
        let mem = MemRef::for_base_imm(base_reg, byte_off as u64);
        self.push_mem(
            encode::enc_str_x_imm(reg, base_reg, byte_off),
            format!(
                "str {}, [{}, #{byte_off}]",
                reg_name(reg),
                reg_name(base_reg)
            ),
            CostRule::Store,
            None,
            &[reg, base_reg],
            Some(mem),
        );
    }

    fn load_byte_imm(&mut self, rt: u8, rn: u8, byte_off: u16) {
        let mem = MemRef::for_base_imm(rn, byte_off as u64);
        self.push_mem(
            encode::enc_ldrb_imm(rt, rn, byte_off),
            format!("ldrb w{rt}, [{}, #{byte_off}]", reg_name(rn)),
            CostRule::Load,
            Some(rt),
            &[rn],
            Some(mem),
        );
    }

    fn addr_of_slot(&mut self, reg: u8, off: usize) {
        self.slot_accesses
            .push((off, regalloc::Touch::Escape, self.words.len(), reg));
        if off >= VIRT_SLOT_BASE {
            self.note_resident_misuse("addr_of_slot", off);
            return;
        }
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        self.push(
            encode::enc_add_imm(reg, base, off, true),
            format!("add {}, {}, #{off}", reg_name(reg), reg_name(base)),
            CostRule::Alu,
            Some(reg),
            &[base],
        );
    }

    fn load_imm_naive(&mut self, reg: u8, value: i64) {
        let bits = value as u64;
        let halves: [(u16, u8); 4] = [
            ((bits & 0xFFFF) as u16, 0),
            (((bits >> 16) & 0xFFFF) as u16, 16),
            (((bits >> 32) & 0xFFFF) as u16, 32),
            (((bits >> 48) & 0xFFFF) as u16, 48),
        ];
        let (h0, _) = halves[0];
        self.push(
            encode::enc_movz(reg, h0, 0, true),
            format!("movz {}, #{h0:#x}", reg_name(reg)),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        for &(imm, shift) in &halves[1..] {
            self.push(
                encode::enc_movk(reg, imm, shift, true),
                format!("movk {}, #{imm:#x}, lsl #{shift}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        }
    }

    fn try_load_imm_one_word(&mut self, reg: u8, value: i64) -> bool {
        let bits = value as u64;
        let halves = [
            (bits & 0xFFFF) as u16,
            ((bits >> 16) & 0xFFFF) as u16,
            ((bits >> 32) & 0xFFFF) as u16,
            ((bits >> 48) & 0xFFFF) as u16,
        ];
        if halves.iter().filter(|h| **h != 0).count() <= 1 {
            return false;
        }
        let inv = !bits;
        let inv_halves = [
            (inv & 0xFFFF) as u16,
            ((inv >> 16) & 0xFFFF) as u16,
            ((inv >> 32) & 0xFFFF) as u16,
            ((inv >> 48) & 0xFFFF) as u16,
        ];
        if inv_halves.iter().filter(|h| **h != 0).count() <= 1 {
            let idx = inv_halves.iter().position(|h| *h != 0).unwrap_or(0);
            let imm = inv_halves[idx];
            let shift = (idx * 16) as u8;
            self.push(
                encode::enc_movn(reg, imm, shift, true),
                if shift == 0 {
                    format!("movn {}, #{imm:#x}", reg_name(reg))
                } else {
                    format!("movn {}, #{imm:#x}, lsl #{shift}", reg_name(reg))
                },
                CostRule::MovWide,
                Some(reg),
                &[],
            );
            return true;
        }
        if let Some(enc) = encode::enc_mov_bitmask_imm(reg, bits) {
            self.push(
                enc,
                format!("mov {}, #{bits:#x}", reg_name(reg)),
                CostRule::Alu,
                Some(reg),
                &[],
            );
            return true;
        }
        false
    }

    fn load_imm(&mut self, reg: u8, value: i64) {
        if !narrow_imm() {
            self.load_imm_naive(reg, value);
            return;
        }
        if wide_imm_forms() && self.try_load_imm_one_word(reg, value) {
            return;
        }
        let bits = value as u64;
        let halves: [(u16, u8); 4] = [
            ((bits & 0xFFFF) as u16, 0),
            (((bits >> 16) & 0xFFFF) as u16, 16),
            (((bits >> 32) & 0xFFFF) as u16, 32),
            (((bits >> 48) & 0xFFFF) as u16, 48),
        ];
        if bits == 0 {
            self.push(
                encode::enc_movz(reg, 0, 0, true),
                format!("movz {}, #0x0", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
            return;
        }
        let first = halves
            .iter()
            .position(|&(imm, _)| imm != 0)
            .expect("bits != 0 implies a non-zero halfword");
        let (imm0, shift0) = halves[first];
        if shift0 == 0 {
            self.push(
                encode::enc_movz(reg, imm0, 0, true),
                format!("movz {}, #{imm0:#x}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        } else {
            self.push(
                encode::enc_movz(reg, imm0, shift0, true),
                format!("movz {}, #{imm0:#x}, lsl #{shift0}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        }
        for &(imm, shift) in &halves[first + 1..] {
            if imm == 0 {
                continue;
            }
            self.push(
                encode::enc_movk(reg, imm, shift, true),
                format!("movk {}, #{imm:#x}, lsl #{shift}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        }
    }

    fn copy_slot_to_slot(&mut self, dst_off: usize, src_off: usize, size: usize) {
        let mut w = 0;
        while w < size {
            match self.frame.reg_at(dst_off + w) {
                Some(_) => {
                    let d = self.def_reg(X_A, dst_off + w);
                    self.load_slot(d, src_off + w);
                    self.store_slot(d, dst_off + w);
                }
                None => {
                    let s = self.use_slot(X_A, src_off + w);
                    self.store_slot(s, dst_off + w);
                }
            }
            w += 8;
        }
    }

    fn narrow_to_width(&mut self, reg: u8, bits: u32, signed: bool) {
        if bits >= 64 {
            return;
        }
        if bfx_narrow() {
            let w = bits as u8;
            let (enc, mnem) = if signed {
                (encode::enc_sbfx(reg, reg, 0, w, true), "sbfx")
            } else {
                (encode::enc_ubfx(reg, reg, 0, w, true), "ubfx")
            };
            self.push(
                enc,
                format!("{mnem} {}, {}, #0, #{bits}", reg_name(reg), reg_name(reg)),
                CostRule::Alu,
                Some(reg),
                &[reg],
            );
            return;
        }
        let shift = (64 - bits) as u8;
        self.push(
            encode::enc_lsl_imm(reg, reg, shift, true),
            format!("lsl {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
            CostRule::Alu,
            Some(reg),
            &[reg],
        );
        if signed {
            self.push(
                encode::enc_asr_imm(reg, reg, shift, true),
                format!("asr {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
                CostRule::Alu,
                Some(reg),
                &[reg],
            );
        } else {
            self.push(
                encode::enc_lsr_imm(reg, reg, shift, true),
                format!("lsr {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
                CostRule::Alu,
                Some(reg),
                &[reg],
            );
        }
    }

    fn branch_target_delta(&self, target_mwir_idx: usize, this_word: usize) -> i32 {
        let target_word = self.word_offsets[target_mwir_idx];
        (target_word as i64 - this_word as i64) as i32 * 4
    }

    fn b_unconditional(&mut self, target_mwir_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_mwir_idx, this_word);
        self.push(
            encode::enc_b(delta),
            format!("b #{delta}"),
            CostRule::Branch,
            None,
            &[],
        );
    }

    fn cbz(&mut self, reg: u8, target_mwir_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_mwir_idx, this_word);
        self.push(
            encode::enc_cbz(reg, delta, true),
            format!("cbz {}, #{delta}", reg_name(reg)),
            CostRule::Branch,
            None,
            &[reg],
        );
    }

    fn load_rodata_addr(&mut self, reg: u8, data_index: usize) {
        let byte_offset = self.rodata.byte_offset(data_index);
        let word_adrp = self.cur_word();
        if adr_addressing() {
            self.push(
                encode::enc_adr(reg, 0),
                format!("adr {}, rodata+{byte_offset:#x}", reg_name(reg)),
                CostRule::Adrp,
                Some(reg),
                &[],
            );
            self.relocs.push(Reloc::RodataAdr {
                word: word_adrp,
                byte_offset,
            });
            return;
        }
        self.push(
            encode::enc_adrp(reg, 0),
            format!("adrp {}, rodata+{byte_offset:#x}", reg_name(reg)),
            CostRule::Adrp,
            Some(reg),
            &[],
        );
        self.push(
            encode::enc_add_imm(reg, reg, 0, true),
            format!(
                "add {}, {}, rodata+{byte_offset:#x}",
                reg_name(reg),
                reg_name(reg)
            ),
            CostRule::Alu,
            Some(reg),
            &[reg],
        );
        self.relocs.push(Reloc::Rodata {
            word_adrp,
            byte_offset,
        });
    }

    fn bl_symbolic_call(&mut self, key: &str, arg_srcs: &[u8]) {
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            format!("bl <{key}>"),
            CostRule::Call,
            Some(0),
            arg_srcs,
        );
        self.relocs.push(Reloc::Call {
            word,
            key: key.to_string(),
        });
    }

    fn emit_block_hit(&mut self, id: u32) {
        self.load_imm_naive(0, id as i64);
        self.bl_symbolic_call("__wrela_block_hit", &[0]);
    }

    fn abort_fixed(&mut self, message: &str) {
        let bytes = message.as_bytes().to_vec();
        let len = bytes.len();
        let idx = self.rodata.intern(bytes);
        self.push(
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; abort Bytes slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        self.load_rodata_addr(X_A, idx);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 0),
            format!("str {}, [sp]  ; Bytes.base", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(0)),
        );
        self.load_imm(X_A, len as i64);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 8),
            format!("str {}, [sp, #8]  ; Bytes.len", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(8)),
        );
        self.push(
            encode::enc_add_imm(0, 31, 0, true),
            "add x0, sp, #0  ; *Bytes".to_string(),
            CostRule::Alu,
            Some(0),
            &[31],
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_abort>".to_string(),
            CostRule::Abort,
            None,
            &[],
        );
        self.relocs.push(Reloc::AbortFixed { word });
    }

    fn checkpoint(&mut self) {
        let addr = wrela_machine::pending::core_word_addr(0);
        self.load_imm(X_A, addr as i64);
        self.push_mem(
            encode::enc_ldr_x_imm(X_B, X_A, 0),
            format!("ldr {}, [{}]", reg_name(X_B), reg_name(X_A)),
            CostRule::Load,
            Some(X_B),
            &[X_A],
            Some(MemRef::for_base_imm(X_A, 0)),
        );
        self.push(
            encode::enc_cbz(X_B, 8, true),
            format!("cbz {}, #8", reg_name(X_B)),
            CostRule::Branch,
            None,
            &[X_B],
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_checkpoint_service>".to_string(),
            CostRule::Call,
            Some(0),
            &[],
        );
        self.relocs.push(Reloc::CheckpointService { word });
    }

    fn abort_val(&mut self, prefix: &str, value_reg: u8, signed: bool, suffix: &str) {
        self.push(
            encode::enc_mov_reg(X_B, value_reg, true),
            format!("mov {}, {}", reg_name(X_B), reg_name(value_reg)),
            CostRule::Alu,
            Some(X_B),
            &[value_reg],
        );
        let prefix_bytes = prefix.as_bytes().to_vec();
        let prefix_len = prefix_bytes.len();
        let prefix_idx = self.rodata.intern(prefix_bytes);
        let suffix_bytes = suffix.as_bytes().to_vec();
        let suffix_len = suffix_bytes.len();
        let suffix_idx = self.rodata.intern(suffix_bytes);
        self.push(
            encode::enc_sub_imm(31, 31, 32, true),
            "sub sp, sp, #32  ; abort_val prefix+suffix Bytes".to_string(),
            CostRule::AbortVal,
            Some(31),
            &[31],
        );
        self.load_rodata_addr(X_A, prefix_idx);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 0),
            format!("str {}, [sp]  ; prefix.base", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(0)),
        );
        self.load_imm(X_A, prefix_len as i64);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 8),
            format!("str {}, [sp, #8]  ; prefix.len", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(8)),
        );
        self.load_rodata_addr(X_A, suffix_idx);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 16),
            format!("str {}, [sp, #16]  ; suffix.base", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(16)),
        );
        self.load_imm(X_A, suffix_len as i64);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 24),
            format!("str {}, [sp, #24]  ; suffix.len", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(24)),
        );
        self.push(
            encode::enc_add_imm(0, 31, 0, true),
            "add x0, sp, #0  ; *prefix".to_string(),
            CostRule::Alu,
            Some(0),
            &[31],
        );
        self.push(
            encode::enc_mov_reg(1, X_B, true),
            format!("mov x1, {}", reg_name(X_B)),
            CostRule::Alu,
            Some(1),
            &[X_B],
        );
        self.load_imm(2, signed as i64);
        self.push(
            encode::enc_add_imm(3, 31, 16, true),
            "add x3, sp, #16  ; *suffix".to_string(),
            CostRule::Alu,
            Some(3),
            &[31],
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_abort_val>".to_string(),
            CostRule::AbortVal,
            None,
            &[],
        );
        self.relocs.push(Reloc::AbortVal { word });
    }
}

fn cond_mnemonic(cond: Cond) -> &'static str {
    match cond {
        Cond::Eq => "eq",
        Cond::Ne => "ne",
        Cond::Cs => "cs",
        Cond::Cc => "cc",
        Cond::Mi => "mi",
        Cond::Pl => "pl",
        Cond::Vs => "vs",
        Cond::Vc => "vc",
        Cond::Hi => "hi",
        Cond::Ls => "ls",
        Cond::Ge => "ge",
        Cond::Lt => "lt",
        Cond::Gt => "gt",
        Cond::Le => "le",
        Cond::Al => "al",
        Cond::Nv => "nv",
    }
}

fn compare_cond(op: BinOp) -> Result<Cond, CodegenError> {
    Ok(match op {
        BinOp::Lt => Cond::Lt,
        BinOp::Le => Cond::Le,
        BinOp::Gt => Cond::Gt,
        BinOp::Ge => Cond::Ge,
        BinOp::Eq => Cond::Eq,
        BinOp::Ne => Cond::Ne,
        other => {
            return Err(CodegenError::internal(format!(
                "`Compare` with a non-ordering op `{}`",
                other.as_str()
            )));
        }
    })
}

#[derive(Debug, Clone, Copy)]
enum SkipKind {
    Cond(Cond),
    Cbz(u8),
    Cbnz(u8),
}

impl FnCtx<'_> {
    fn emit_skip(&mut self, _kind: SkipKind) -> usize {
        let w = self.cur_word();
        self.words
            .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));
        w
    }

    fn patch_skip(&mut self, word: usize, kind: SkipKind) {
        let target = self.cur_word();
        let delta = (target as i64 - word as i64) as i32 * 4;
        let (enc, text, srcs, flags) = match kind {
            SkipKind::Cond(c) => {
                let flags = match c {
                    Cond::Al | Cond::Nv => FlagEffect::None,
                    _ => FlagEffect::Read,
                };
                (
                    encode::enc_b_cond(c, delta),
                    format!("b.{} #{delta}", cond_mnemonic(c)),
                    Vec::<u8>::new(),
                    flags,
                )
            }
            SkipKind::Cbz(r) => (
                encode::enc_cbz(r, delta, true),
                format!("cbz {}, #{delta}", reg_name(r)),
                vec![r],
                FlagEffect::None,
            ),
            SkipKind::Cbnz(r) => (
                encode::enc_cbnz(r, delta, true),
                format!("cbnz {}, #{delta}", reg_name(r)),
                vec![r],
                FlagEffect::None,
            ),
        };
        self.words[word] =
            EmittedWord::new(enc, text, CostRule::Branch, None, &srcs).with_flags(flags);
    }

    fn check_int_range_or_abort(&mut self, value_reg: u8, bits: u32, signed: bool, message: &str) {
        assert!(
            bits < 64,
            "check_int_range_or_abort is the narrow-width check; {bits}-bit \
             values use the flag-based scheme"
        );
        if !mask_check() {
            let (min, max) = int_bounds_for(bits, signed);
            self.check_bounds_i64_or_abort(value_reg, min, max, message);
            return;
        }
        if signed {
            self.push(
                encode::enc_sbfx(X_D, value_reg, 0, bits as u8, true),
                format!(
                    "sbfx {}, {}, #0, #{bits}",
                    reg_name(X_D),
                    reg_name(value_reg)
                ),
                CostRule::Alu,
                Some(X_D),
                &[value_reg],
            );
            self.cmp_reg(value_reg, X_D);
            let skip = self.emit_skip(SkipKind::Cond(Cond::Eq));
            self.abort_fixed(message);
            self.patch_skip(skip, SkipKind::Cond(Cond::Eq));
            return;
        }
        let mask = !((1u64 << bits) - 1);
        let Some(enc) = encode::enc_tst_imm(value_reg, mask) else {
            let (min, max) = int_bounds_for(bits, signed);
            self.check_bounds_i64_or_abort(value_reg, min, max, message);
            return;
        };
        self.push_flags(
            enc,
            format!("tst {}, #{mask:#x}", reg_name(value_reg)),
            CostRule::Alu,
            None,
            &[value_reg],
            FlagEffect::Write,
        );
        let skip = self.emit_skip(SkipKind::Cond(Cond::Eq));
        self.abort_fixed(message);
        self.patch_skip(skip, SkipKind::Cond(Cond::Eq));
    }

    fn check_bounds_i64_or_abort(&mut self, value_reg: u8, min: i64, max: i64, message: &str) {
        self.load_imm(X_D, min);
        self.cmp_reg(value_reg, X_D);
        let skip1 = self.emit_skip(SkipKind::Cond(Cond::Ge));
        self.abort_fixed(message);
        self.patch_skip(skip1, SkipKind::Cond(Cond::Ge));
        self.load_imm(X_D, max);
        self.cmp_reg(value_reg, X_D);
        let skip2 = self.emit_skip(SkipKind::Cond(Cond::Le));
        self.abort_fixed(message);
        self.patch_skip(skip2, SkipKind::Cond(Cond::Le));
    }

    fn check_flags_or_abort(&mut self, fail_cond: Cond, message: &str) {
        let pass = fail_cond.invert();
        let skip = self.emit_skip(SkipKind::Cond(pass));
        self.abort_fixed(message);
        self.patch_skip(skip, SkipKind::Cond(pass));
    }
}

fn emit_one(inst: &Inst, f: &MwirFn, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    match inst {
        Inst::ConstInt { dst, ty, value } => {
            if is_float(ty) {
                return Err(CodegenError::internal("`ConstInt` with a float type"));
            }
            let off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_A, off);
            ctx.load_imm(d, *value as i64);
            ctx.store_slot(d, off);
        }
        Inst::ConstBool { dst, value } => {
            let off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_A, off);
            ctx.load_imm(d, if *value { 1 } else { 0 });
            ctx.store_slot(d, off);
        }
        Inst::ConstFloat { .. } => {
            return Err(CodegenError::unimplemented(
                "floating-point constants (no FP/SIMD encoder subset exists)",
            ));
        }
        Inst::ConstChar { dst, value } => {
            let off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_A, off);
            ctx.load_imm(d, *value as u32 as i64);
            ctx.store_slot(d, off);
        }
        Inst::ConstUnit { dst } => {
            let off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_A, off);
            ctx.load_imm(d, 0);
            ctx.store_slot(d, off);
        }
        Inst::ConstText { .. } => {
            return Err(CodegenError::unimplemented(
                "`Static[Str]`/`Static[Bytes[N]]` values (mwir::size_of itself has no layout for a bare `Str` yet)",
            ));
        }
        Inst::Copy { dst, src } => {
            let size = ctx.frame.size_of_temp(*dst);
            ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*src), size);
        }
        Inst::MakeAggregate { dst, elems } => {
            let dst_off = ctx.frame.off(*dst);
            let mut cur = 0usize;
            for e in elems {
                let sz = ctx.frame.size_of_temp(*e);
                ctx.copy_slot_to_slot(dst_off + cur, ctx.frame.off(*e), sz);
                cur += sz;
            }
        }
        Inst::FormatScalar {
            dst,
            src,
            src_ty,
            capacity,
        } => emit_format_scalar(ctx, *dst, *src, src_ty, *capacity)?,
        Inst::StringConcat {
            dst,
            lhs,
            rhs,
            lhs_cap,
            rhs_cap,
        } => emit_string_concat(ctx, *dst, *lhs, *rhs, *lhs_cap, *rhs_cap),
        Inst::Project { dst, base, index } => {
            let base_ty = f.temp_types[base.0].clone();
            if matches!(base_ty, Type::Own(..)) {
                let payload_ty = unwrap_own_ref(&base_ty);
                let (off, size) = field_offset_size(payload_ty, *index, ctx.layout)?;
                let b = ctx.use_slot(X_A, ctx.frame.off(*base));
                let dst_off = ctx.frame.off(*dst);
                let mut w = 0;
                while w < size {
                    let d = ctx.def_reg(X_B, dst_off + w);
                    ctx.load_ptr(d, b, off + w);
                    ctx.store_slot(d, dst_off + w);
                    w += 8;
                }
            } else {
                let (off, size) = field_offset_size(&base_ty, *index, ctx.layout)?;
                ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*base) + off, size);
            }
        }
        Inst::SetField { base, index, value } => {
            let base_ty = f.temp_types[base.0].clone();
            if matches!(base_ty, Type::Own(..)) {
                let payload_ty = unwrap_own_ref(&base_ty);
                let (off, size) = field_offset_size(payload_ty, *index, ctx.layout)?;
                let b = ctx.use_slot(X_A, ctx.frame.off(*base));
                let src_off = ctx.frame.off(*value);
                let mut w = 0;
                while w < size {
                    let s = ctx.use_slot(X_B, src_off + w);
                    ctx.store_ptr(s, b, off + w);
                    w += 8;
                }
            } else {
                let (off, size) = field_offset_size(&base_ty, *index, ctx.layout)?;
                ctx.copy_slot_to_slot(ctx.frame.off(*base) + off, ctx.frame.off(*value), size);
            }
        }
        Inst::IndexGet {
            dst,
            base,
            index,
            len,
        } => {
            let base_ty = f.temp_types[base.0].clone();
            let elem_ty = array_elem_type(&base_ty)?;
            let elem_size =
                mwir::size_of(&elem_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
            emit_index_addr(
                ctx,
                ctx.frame.off(*base),
                ctx.frame.off(*index),
                *len,
                elem_size,
                X_C,
            );
            let dst_off = ctx.frame.off(*dst);
            let mut w = 0;
            while w < elem_size {
                let d = ctx.def_reg(X_F, dst_off + w);
                ctx.load_ptr(d, X_C, w);
                ctx.store_slot(d, dst_off + w);
                w += 8;
            }
        }
        Inst::IndexSet {
            base,
            index,
            value,
            len,
        } => {
            let base_ty = f.temp_types[base.0].clone();
            let elem_ty = array_elem_type(&base_ty)?;
            let elem_size =
                mwir::size_of(&elem_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
            emit_index_addr(
                ctx,
                ctx.frame.off(*base),
                ctx.frame.off(*index),
                *len,
                elem_size,
                X_C,
            );
            let val_off = ctx.frame.off(*value);
            let mut w = 0;
            while w < elem_size {
                let s = ctx.use_slot(X_F, val_off + w);
                ctx.store_ptr(s, X_C, w);
                w += 8;
            }
        }
        Inst::PlacedIndexGet {
            dst,
            base,
            field_offset,
            index,
            len,
            elem_stride,
            ty,
        } => {
            emit_placed_index_addr(
                ctx,
                ctx.frame.off(*base),
                *field_offset,
                ctx.frame.off(*index),
                *len,
                *elem_stride,
                X_C,
            );
            let width = mmio_access_width(ty, 0)?;
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_B, dst_off);
            let (enc, mnem) = match width {
                1 => (encode::enc_ldrb_imm(d, X_C, 0), "ldrb"),
                2 => (encode::enc_ldrh_imm(d, X_C, 0), "ldrh"),
                4 => (encode::enc_ldr_w_imm(d, X_C, 0), "ldr"),
                _ => (encode::enc_ldr_x_imm(d, X_C, 0), "ldr"),
            };
            let rt = if width == 8 {
                reg_name(d)
            } else {
                format!("w{d}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #0]", reg_name(X_C)),
                CostRule::Load,
                Some(d),
                &[X_C],
            );
            ctx.store_slot(d, dst_off);
        }
        Inst::PlacedIndexSet {
            base,
            field_offset,
            index,
            value,
            len,
            elem_stride,
            ty,
        } => {
            emit_placed_index_addr(
                ctx,
                ctx.frame.off(*base),
                *field_offset,
                ctx.frame.off(*index),
                *len,
                *elem_stride,
                X_C,
            );
            let width = mmio_access_width(ty, 0)?;
            let v = ctx.use_slot(X_B, ctx.frame.off(*value));
            let (enc, mnem) = match width {
                1 => (encode::enc_strb_imm(v, X_C, 0), "strb"),
                2 => (encode::enc_strh_imm(v, X_C, 0), "strh"),
                4 => (encode::enc_str_w_imm(v, X_C, 0), "str"),
                _ => (encode::enc_str_x_imm(v, X_C, 0), "str"),
            };
            let rt = if width == 8 {
                reg_name(v)
            } else {
                format!("w{v}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #0]", reg_name(X_C)),
                CostRule::Store,
                None,
                &[v, X_C],
            );
        }
        Inst::BytesIndexGet { dst, base, index } => {
            emit_bytes_index_addr(ctx, ctx.frame.off(*base), ctx.frame.off(*index), X_C)?;
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_B, dst_off);
            ctx.load_byte_imm(d, X_C, 0);
            ctx.store_slot(d, dst_off);
        }
        Inst::MakeEnum { dst, tag, payload } => {
            let dst_off = ctx.frame.off(*dst);
            let dst_ty = f.temp_types[dst.0].clone();
            if is_option_group_id(&dst_ty) {
                if *tag == 0 {
                    let d = ctx.def_reg(X_A, dst_off);
                    ctx.load_imm(d, 0);
                    ctx.store_slot(d, dst_off);
                } else {
                    let p = payload.first().copied().ok_or_else(|| {
                        CodegenError::internal("Some(GroupId) MakeEnum with no payload")
                    })?;
                    let sz = ctx.frame.size_of_temp(p);
                    ctx.copy_slot_to_slot(dst_off, ctx.frame.off(p), sz);
                }
            } else {
                let d = ctx.def_reg(X_A, dst_off);
                ctx.load_imm(d, *tag as i64);
                ctx.store_slot(d, dst_off);
                let mut cur = 8usize;
                for p in payload {
                    let sz = ctx.frame.size_of_temp(*p);
                    ctx.copy_slot_to_slot(dst_off + cur, ctx.frame.off(*p), sz);
                    cur += sz;
                }
            }
        }
        Inst::EnumTag { dst, src } => {
            let src_ty = f.temp_types[src.0].clone();
            if is_option_group_id(&src_ty) {
                let s = ctx.use_slot(X_A, ctx.frame.off(*src));
                ctx.push_flags(
                    encode::enc_cmp_imm(s, 0, true),
                    format!("cmp {}, #0", reg_name(s)),
                    CostRule::Alu,
                    None,
                    &[s],
                    FlagEffect::Write,
                );
                let dst_off = ctx.frame.off(*dst);
                let d = ctx.def_reg(X_A, dst_off);
                ctx.push_flags(
                    encode::enc_cset(d, Cond::Ne, true),
                    format!("cset {}, ne", reg_name(d)),
                    CostRule::Alu,
                    Some(d),
                    &[],
                    FlagEffect::Read,
                );
                ctx.store_slot(d, dst_off);
            } else {
                let s = ctx.use_slot(X_A, ctx.frame.off(*src));
                ctx.store_slot(s, ctx.frame.off(*dst));
            }
        }
        Inst::EnumPayload { dst, src, index } => {
            let src_ty = f.temp_types[src.0].clone();
            let off = enum_payload_offset(&src_ty, *index, ctx.layout)?;
            let size = ctx.frame.size_of_temp(*dst);
            ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*src) + off, size);
        }
        Inst::ArithChecked {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort,
        } => emit_arith_checked(ctx, *op, ty, *lhs, *rhs, *dst, abort)?,
        Inst::ArithWrapping {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => emit_arith_wrapping(ctx, *op, ty, *lhs, *rhs, *dst)?,
        Inst::DivRem {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort_zero,
            abort_overflow,
        } => emit_div_rem(ctx, *op, ty, *lhs, *rhs, *dst, abort_zero, abort_overflow)?,
        Inst::Shift {
            dst,
            op,
            ty,
            lhs,
            rhs,
            bits,
            lost,
        } => emit_shift(ctx, *op, ty, *lhs, *rhs, *bits, lost.as_deref(), *dst)?,
        Inst::Bitwise {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => {
            if is_float(ty) {
                return Err(CodegenError::internal("`Bitwise` with a float type"));
            }
            let a = ctx.use_slot(X_A, ctx.frame.off(*lhs));
            let b = ctx.use_slot(X_B, ctx.frame.off(*rhs));
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_C, dst_off);
            let (enc, mnem) = match op {
                BinOp::BitAnd => (encode::enc_and_reg(d, a, b, true), "and"),
                BinOp::BitOr => (encode::enc_orr_reg(d, a, b, true), "orr"),
                BinOp::BitXor => (encode::enc_eor_reg(d, a, b, true), "eor"),
                other => {
                    return Err(CodegenError::internal(format!(
                        "`Bitwise` with a non-bitwise op `{}`",
                        other.as_str()
                    )));
                }
            };
            ctx.push(
                enc,
                format!("{mnem} {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
                CostRule::Alu,
                None,
                &[],
            );
            ctx.store_slot(d, dst_off);
        }
        Inst::Compare {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => {
            if is_float(ty) {
                return Err(CodegenError::unimplemented("floating-point comparison"));
            }
            let a = ctx.use_slot(X_A, ctx.frame.off(*lhs));
            let b = ctx.use_slot(X_B, ctx.frame.off(*rhs));
            ctx.cmp_reg(a, b);
            let cond = compare_cond(*op)?;
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_C, dst_off);
            ctx.push_flags(
                encode::enc_cset(d, cond, true),
                format!("cset {}, {}", reg_name(d), cond_mnemonic(cond)),
                CostRule::Alu,
                Some(d),
                &[],
                FlagEffect::Read,
            );
            ctx.store_slot(d, dst_off);
        }
        Inst::Neg {
            dst,
            ty,
            src,
            abort,
        } => {
            if is_float(ty) {
                return Err(CodegenError::unimplemented("floating-point negation"));
            }
            let (_, signed) = int_shape(ty)
                .ok_or_else(|| CodegenError::internal(format!("`Neg` on non-integer {ty:?}")))?;
            if !signed {
                return Err(CodegenError::internal("`Neg` on an unsigned type"));
            }
            let (min, _) = int_bounds_i64(ty).unwrap();
            let a = ctx.use_slot(X_A, ctx.frame.off(*src));
            ctx.load_imm(X_D, min);
            ctx.cmp_reg(a, X_D);
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ne));
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_C, dst_off);
            ctx.push(
                encode::enc_sub_reg(d, X_ZR, a, true),
                format!("neg {}, {}", reg_name(d), reg_name(a)),
                CostRule::Alu,
                Some(d),
                &[X_ZR, a],
            );
            ctx.store_slot(d, dst_off);
        }
        Inst::BitNot { dst, ty, src } => {
            if is_float(ty) {
                return Err(CodegenError::internal("`BitNot` with a float type"));
            }
            let (bits, signed) = int_shape(ty)
                .ok_or_else(|| CodegenError::internal(format!("`BitNot` on non-integer {ty:?}")))?;
            let a = ctx.use_slot(X_A, ctx.frame.off(*src));
            ctx.load_imm(X_D, -1);
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_C, dst_off);
            ctx.push(
                encode::enc_eor_reg(d, a, X_D, true),
                format!("eor {}, {}, {}", reg_name(d), reg_name(a), reg_name(X_D)),
                CostRule::Alu,
                Some(d),
                &[a, X_D],
            );
            ctx.narrow_to_width(d, bits, signed);
            ctx.store_slot(d, dst_off);
        }
        Inst::Convert {
            dst,
            ty,
            src,
            abort,
        } => emit_convert(ctx, f, ty, *src, *dst, abort)?,
        Inst::Not { dst, src } => {
            let a = ctx.use_slot(X_A, ctx.frame.off(*src));
            ctx.push_flags(
                encode::enc_cmp_reg(a, X_ZR, true),
                format!("cmp {}, xzr", reg_name(a)),
                CostRule::Alu,
                None,
                &[a, X_ZR],
                FlagEffect::Write,
            );
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_C, dst_off);
            ctx.push_flags(
                encode::enc_cset(d, Cond::Eq, true),
                format!("cset {}, eq", reg_name(d)),
                CostRule::Alu,
                Some(d),
                &[],
                FlagEffect::Read,
            );
            ctx.store_slot(d, dst_off);
        }
        Inst::BoolAnd { dst, lhs, rhs } => {
            let a = ctx.use_slot(X_A, ctx.frame.off(*lhs));
            let b = ctx.use_slot(X_B, ctx.frame.off(*rhs));
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_C, dst_off);
            ctx.and_reg(d, a, b);
            ctx.store_slot(d, dst_off);
        }
        Inst::Jump { target } => {
            if !ctx.elide_branch {
                ctx.b_unconditional(*target)
            }
        }
        Inst::JumpIfFalse { cond, target } => {
            let c = ctx.use_slot(X_A, ctx.frame.off(*cond));
            ctx.cbz(c, *target);
        }
        Inst::Call {
            dst,
            write_backs,
            key,
            args,
        } => {
            if args.len() > 8 {
                return Err(CodegenError::unimplemented("more than 8 call arguments"));
            }
            let mut by_ptr: BTreeSet<usize> = write_backs.iter().map(|(i, _)| *i).collect();
            for (i, arg) in args.iter().enumerate() {
                let arg_ty = &f.temp_types[arg.0];
                if is_aggregate(arg_ty) {
                    by_ptr.insert(i);
                }
            }
            for (i, arg) in args.iter().enumerate() {
                if i > 8 {
                    return Err(CodegenError::unimplemented("more than 8 call arguments"));
                }
                if by_ptr.contains(&i) {
                    ctx.addr_of_slot(i as u8, ctx.frame.off(*arg));
                } else {
                    ctx.load_slot(i as u8, ctx.frame.off(*arg));
                }
            }
            let dst_ty = f.temp_types[dst.0].clone();
            if is_aggregate(&dst_ty) {
                ctx.addr_of_slot(8, ctx.frame.off(*dst));
            }
            let mut arg_srcs: Vec<u8> = (0..args.len().min(8)).map(|i| i as u8).collect();
            if is_aggregate(&dst_ty) {
                arg_srcs.push(8);
            }
            ctx.bl_symbolic_call(key, &arg_srcs);
            if !is_aggregate(&dst_ty) {
                ctx.store_slot(0, ctx.frame.off(*dst));
            }
        }
        Inst::Return { value } => {
            if let Some(v) = value {
                if is_aggregate(&f.ret) {
                    let ret_ptr_off = ctx.frame.ret_ptr_off.ok_or_else(|| {
                        CodegenError::internal("`Return` with a value but no ret_ptr slot")
                    })?;
                    ctx.load_slot(X_A, ret_ptr_off);
                    let size = ctx.frame.size_of_temp(*v);
                    let v_off = ctx.frame.off(*v);
                    let mut w = 0;
                    while w < size {
                        ctx.load_slot(X_B, v_off + w);
                        ctx.store_ptr(X_B, X_A, w);
                        w += 8;
                    }
                } else {
                    ctx.load_slot(0, ctx.frame.off(*v));
                }
            }
            if !ctx.elide_branch {
                ctx.b_unconditional(f.body.len());
            }
        }
        Inst::AssertFail { message } => {
            let msg = match message {
                Some(m) => format!("assertion failed: {m}"),
                None => "assertion failed".to_string(),
            };
            ctx.abort_fixed(&msg);
        }

        Inst::MmioRead {
            dst,
            base,
            offset,
            ty,
        } => {
            let width = mmio_access_width(ty, *offset)?;
            let b = ctx.use_slot(X_A, ctx.frame.off(*base));
            let off = *offset as u16;
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_B, dst_off);
            let (enc, mnem) = match width {
                1 => (encode::enc_ldrb_imm(d, b, off), "ldrb"),
                2 => (encode::enc_ldrh_imm(d, b, off), "ldrh"),
                4 => (encode::enc_ldr_w_imm(d, b, off), "ldr"),
                _ => (encode::enc_ldr_x_imm(d, b, off), "ldr"),
            };
            let rt = if width == 8 {
                reg_name(d)
            } else {
                format!("w{d}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #{off}]", reg_name(b)),
                CostRule::Load,
                Some(d),
                &[b],
            );
            ctx.store_slot(d, dst_off);
        }
        Inst::LoadIrqVector { dst, driver } => {
            let word = ctx.words.len();
            let dst_off = ctx.frame.off(*dst);
            let d = ctx.def_reg(X_A, dst_off);
            ctx.load_imm_naive(d, 0);
            if let Some(ew) = ctx.words.get_mut(word) {
                ew.text = format!("irq-vector[{}] {}", driver, reg_name(d));
            }
            ctx.relocs.push(Reloc::IrqVector {
                word,
                driver: driver.clone(),
            });
            ctx.store_slot(d, dst_off);
        }
        Inst::InterruptCellLoadAcquire {
            dst,
            field_off,
            width,
        } => {
            emit_interrupt_cell_addr(ctx, *field_off)?;
            let mem = Some(interrupt_cell_memref(*field_off));
            match *width {
                4 => {
                    ctx.push_mem(
                        encode::enc_ldar_w(X_B, X_A),
                        format!("ldar w{}, [{}]", X_B, reg_name(X_A)),
                        CostRule::LoadAcquire,
                        Some(X_B),
                        &[X_A],
                        mem,
                    );
                }
                8 => {
                    ctx.push_mem(
                        encode::enc_ldar_x(X_B, X_A),
                        format!("ldar {}, [{}]", reg_name(X_B), reg_name(X_A)),
                        CostRule::LoadAcquire,
                        Some(X_B),
                        &[X_A],
                        mem,
                    );
                }
                w => {
                    return Err(CodegenError::internal(format!(
                        "InterruptCellLoadAcquire width {w}"
                    )));
                }
            }
            ctx.store_slot(X_B, ctx.frame.off(*dst));
        }
        Inst::InterruptCellStoreRelease {
            field_off,
            width,
            value,
        } => {
            emit_interrupt_cell_addr(ctx, *field_off)?;
            ctx.load_slot(X_B, ctx.frame.off(*value));
            let mem = Some(interrupt_cell_memref(*field_off));
            match *width {
                4 => {
                    ctx.push_mem(
                        encode::enc_stlr_w(X_B, X_A),
                        format!("stlr w{}, [{}]", X_B, reg_name(X_A)),
                        CostRule::StoreRelease,
                        None,
                        &[X_A, X_B],
                        mem,
                    );
                }
                8 => {
                    ctx.push_mem(
                        encode::enc_stlr_x(X_B, X_A),
                        format!("stlr {}, [{}]", reg_name(X_B), reg_name(X_A)),
                        CostRule::StoreRelease,
                        None,
                        &[X_A, X_B],
                        mem,
                    );
                }
                w => {
                    return Err(CodegenError::internal(format!(
                        "InterruptCellStoreRelease width {w}"
                    )));
                }
            }
        }
        Inst::InterruptCellSwapAcquire {
            dst,
            field_off,
            width,
            value,
        } => {
            let value_off = ctx.frame.off(*value);
            let dst_off = ctx.frame.off(*dst);
            emit_interrupt_cell_rmw(ctx, *field_off, *width, value_off, InterruptCellRmw::Swap)?;
            ctx.store_slot(X_C, dst_off);
        }
        Inst::InterruptCellFetchOrRelease {
            dst,
            field_off,
            width,
            value,
        } => {
            let value_off = ctx.frame.off(*value);
            let dst_off = ctx.frame.off(*dst);
            emit_interrupt_cell_rmw(
                ctx,
                *field_off,
                *width,
                value_off,
                InterruptCellRmw::FetchOr,
            )?;
            ctx.store_slot(X_C, dst_off);
        }
        Inst::Dmb { option } => {
            if omit_dmb() {
                return Ok(());
            }
            let (enc, mnem) = match option.as_str() {
                "ishst" => (encode::enc_dmb_ishst(), "dmb ishst"),
                "ishld" => (encode::enc_dmb_ishld(), "dmb ishld"),
                other => {
                    return Err(CodegenError::internal(format!(
                        "unknown Dmb option `{other}` (expected ishst|ishld)"
                    )));
                }
            };
            ctx.push(enc, mnem.to_string(), CostRule::Barrier, None, &[]);
        }
        Inst::Wake { driver } => {
            let word = ctx.words.len();
            ctx.load_imm_naive(X_A, 0);
            if let Some(ew) = ctx.words.get_mut(word) {
                ew.text = format!("wake-pending[{}] {}", driver, reg_name(X_A));
            }
            ctx.relocs.push(Reloc::WakePending {
                word,
                driver: driver.clone(),
            });
            ctx.load_imm(X_B, 1);
            ctx.push(
                encode::enc_str_x_imm(X_B, X_A, 0),
                format!("str {}, [{}]", reg_name(X_B), reg_name(X_A)),
                CostRule::Store,
                None,
                &[X_B, X_A],
            );
        }
        Inst::Now { dst } => {
            emit_now(*dst, ctx);
        }
        Inst::Entropy { dst, n } => emit_entropy(*dst, *n, ctx)?,
        Inst::SlotMapMint { map } => {
            emit_slotmap_mint_id(*map, ctx)?;
        }
        Inst::MmioWrite {
            base,
            offset,
            ty,
            value,
        } => {
            let width = mmio_access_width(ty, *offset)?;
            let b = ctx.use_slot(X_A, ctx.frame.off(*base));
            let v = ctx.use_slot(X_B, ctx.frame.off(*value));
            let off = *offset as u16;
            let (enc, mnem) = match width {
                1 => (encode::enc_strb_imm(v, b, off), "strb"),
                2 => (encode::enc_strh_imm(v, b, off), "strh"),
                4 => (encode::enc_str_w_imm(v, b, off), "str"),
                _ => (encode::enc_str_x_imm(v, b, off), "str"),
            };
            let rt = if width == 8 {
                reg_name(v)
            } else {
                format!("w{v}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #{off}]", reg_name(b)),
                CostRule::Store,
                None,
                &[v, b],
            );
        }
        Inst::MemLoad {
            dst,
            base,
            offset,
            width,
        } => {
            emit_mem_load(ctx, *dst, *base, *offset, *width)?;
        }
        Inst::MemStore {
            base,
            offset,
            value,
            width,
        } => {
            emit_mem_store(ctx, *base, *offset, *value, *width)?;
        }
        Inst::PtrOffset { dst, base, offset } => {
            let b = ctx.use_slot(X_A, ctx.frame.off(*base));
            let dst_off = ctx.frame.off(*dst);
            if *offset == 0 {
                ctx.store_slot(b, dst_off);
            } else {
                ctx.load_imm(X_B, *offset as i64);
                let d = ctx.def_reg(X_C, dst_off);
                ctx.add_reg(d, b, X_B);
                ctx.store_slot(d, dst_off);
            }
        }
        Inst::TurnAddrFromId { dst, id } => {
            ctx.load_slot(X_A, ctx.frame.off(*id));
            push_turn_addr_from_id(ctx, X_A, X_B);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::Abort { message } => {
            ctx.abort_fixed(message);
        }
    }
    Ok(())
}

fn push_turn_addr_from_id(ctx: &mut FnCtx, id_reg: u8, scratch: u8) {
    ctx.push(
        encode::enc_sub_imm(id_reg, id_reg, 1, true),
        format!("sub {}, {}, #1", reg_name(id_reg), reg_name(id_reg)),
        CostRule::Alu,
        Some(id_reg),
        &[id_reg],
    );
    let word = ctx.cur_word();
    ctx.load_imm_naive(scratch, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-stride {}", reg_name(scratch));
    }
    ctx.relocs.push(Reloc::TurnStride { word });
    ctx.mul_reg(id_reg, id_reg, scratch);
    let word = ctx.cur_word();
    ctx.load_imm_naive(scratch, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turns-base {}", reg_name(scratch));
    }
    ctx.relocs.push(Reloc::TurnsBase { word });
    ctx.add_reg(id_reg, scratch, id_reg);
}

fn emit_mem_addr(ctx: &mut FnCtx, base: Temp, offset: u64) -> u8 {
    if offset == 0 {
        return ctx.use_slot(X_A, ctx.frame.off(base));
    }
    ctx.load_slot(X_A, ctx.frame.off(base));
    ctx.load_imm(X_B, offset as i64);
    ctx.add_reg(X_A, X_A, X_B);
    X_A
}

fn emit_mem_load(
    ctx: &mut FnCtx,
    dst: Temp,
    base: Temp,
    offset: u64,
    width: u8,
) -> Result<(), CodegenError> {
    let addr = emit_mem_addr(ctx, base, offset);
    let dst_off = ctx.frame.off(dst);
    let d = ctx.def_reg(X_B, dst_off);
    let (enc, mnem) = match width {
        1 => (encode::enc_ldrb_imm(d, addr, 0), "ldrb"),
        2 => (encode::enc_ldrh_imm(d, addr, 0), "ldrh"),
        4 => (encode::enc_ldr_w_imm(d, addr, 0), "ldr"),
        8 => (encode::enc_ldr_x_imm(d, addr, 0), "ldr"),
        w => {
            return Err(CodegenError::internal(format!(
                "MemLoad width {w} (want 1/2/4/8)"
            )));
        }
    };
    let rt = if width == 8 {
        reg_name(d)
    } else {
        format!("w{d}")
    };
    ctx.push(
        enc,
        format!("{mnem} {rt}, [{}]", reg_name(addr)),
        CostRule::Load,
        Some(d),
        &[addr],
    );
    ctx.store_slot(d, dst_off);
    Ok(())
}

fn emit_mem_store(
    ctx: &mut FnCtx,
    base: Temp,
    offset: u64,
    value: Temp,
    width: u8,
) -> Result<(), CodegenError> {
    let addr = emit_mem_addr(ctx, base, offset);
    let v = ctx.use_slot(X_B, ctx.frame.off(value));
    let (enc, mnem) = match width {
        1 => (encode::enc_strb_imm(v, addr, 0), "strb"),
        2 => (encode::enc_strh_imm(v, addr, 0), "strh"),
        4 => (encode::enc_str_w_imm(v, addr, 0), "str"),
        8 => (encode::enc_str_x_imm(v, addr, 0), "str"),
        w => {
            return Err(CodegenError::internal(format!(
                "MemStore width {w} (want 1/2/4/8)"
            )));
        }
    };
    let rt = if width == 8 {
        reg_name(v)
    } else {
        format!("w{v}")
    };
    ctx.push(
        enc,
        format!("{mnem} {rt}, [{}]", reg_name(addr)),
        CostRule::Store,
        None,
        &[v, addr],
    );
    Ok(())
}

fn mmio_access_width(ty: &Type, offset: u64) -> Result<u16, CodegenError> {
    let width = match strip_wrappers(ty) {
        Type::U8 => 1,
        Type::U16 => 2,
        Type::U32 => 4,
        Type::U64 | Type::Usize => 8,
        other => {
            return Err(CodegenError::unimplemented(&format!(
                "an MMIO register declared `{}`: this backend emits only the four unsigned \
                 widths (`u8`/`u16`/`u32`/`u64`/`usize`); a signed register would need a \
                 sign-extending load this encoder does not have",
                crate::sema::types::render_type(&other)
            )));
        }
    };
    if offset % width as u64 != 0 {
        return Err(CodegenError::internal(format!(
            "an MMIO register at offset {offset:#x} is not {width}-byte aligned ( \
             `types::check_layouts` already refuses this)"
        )));
    }
    if offset / width as u64 >= 4096 {
        return Err(CodegenError::unimplemented(&format!(
            "an MMIO register at offset {offset:#x}: the unsigned-immediate load/store encoder \
             reaches {} bytes at this width, and no base-plus-register addressing form is \
             emitted yet. That offset",
            4095 * width as u64
        )));
    }
    Ok(width)
}

fn array_elem_type(base_ty: &Type) -> Result<Type, CodegenError> {
    match strip_wrappers(base_ty) {
        Type::Array(elem, _) => Ok((**elem).clone()),
        other => Err(CodegenError::internal(format!(
            "indexing a non-array type: {other:?}"
        ))),
    }
}

fn emit_format_scalar(
    ctx: &mut FnCtx,
    dst: Temp,
    src: Temp,
    src_ty: &Type,
    capacity: usize,
) -> Result<(), CodegenError> {
    let dst_off = ctx.frame.off(dst);
    let src_off = ctx.frame.off(src);
    for i in 0..=capacity {
        ctx.load_imm(X_A, 0);
        ctx.store_slot(X_A, dst_off + 8 * i);
    }
    match src_ty {
        Type::Bool => {
            ctx.load_slot(X_A, src_off);
            let to_false = ctx.emit_skip(SkipKind::Cbz(X_A));
            ctx.load_imm(X_A, 4);
            ctx.store_slot(X_A, dst_off);
            for (i, b) in b"true".iter().enumerate() {
                ctx.load_imm(X_A, i128::from(*b) as i64);
                ctx.store_slot(X_A, dst_off + 8 * (1 + i));
            }
            let done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(to_false, SkipKind::Cbz(X_A));
            ctx.load_imm(X_A, 5);
            ctx.store_slot(X_A, dst_off);
            for (i, b) in b"false".iter().enumerate() {
                ctx.load_imm(X_A, i128::from(*b) as i64);
                ctx.store_slot(X_A, dst_off + 8 * (1 + i));
            }
            ctx.patch_skip(done, SkipKind::Cond(Cond::Al));
            Ok(())
        }
        Type::Char => {
            ctx.load_slot(X_A, src_off);
            ctx.load_imm(X_B, 0x80);
            ctx.cmp_reg(X_A, X_B);
            let not_ascii = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            ctx.load_imm(X_B, 1);
            ctx.store_slot(X_B, dst_off);
            ctx.store_slot(X_A, dst_off + 8);
            let done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(not_ascii, SkipKind::Cond(Cond::Cs));
            ctx.load_imm(X_B, 0x800);
            ctx.cmp_reg(X_A, X_B);
            let not_2 = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            ctx.push(
                encode::enc_lsr_imm(X_C, X_A, 6, true),
                format!("lsr {}, {}, #6", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_A],
            );
            ctx.load_imm(X_D, 0xC0);
            ctx.orr_reg(X_C, X_C, X_D);
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_E, X_A, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_E, X_E, X_D);
            ctx.load_imm(X_B, 2);
            ctx.store_slot(X_B, dst_off);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.store_slot(X_E, dst_off + 16);
            let done2 = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(not_2, SkipKind::Cond(Cond::Cs));
            ctx.load_imm(X_B, 0x10000);
            ctx.cmp_reg(X_A, X_B);
            let not_3 = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            ctx.push(
                encode::enc_lsr_imm(X_C, X_A, 12, true),
                format!("lsr {}, {}, #12", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_A],
            );
            ctx.load_imm(X_D, 0xE0);
            ctx.orr_reg(X_C, X_C, X_D);
            ctx.push(
                encode::enc_lsr_imm(X_E, X_A, 6, true),
                format!("lsr {}, {}, #6", reg_name(X_E), reg_name(X_A)),
                CostRule::Alu,
                Some(X_E),
                &[X_A],
            );
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_E, X_E, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_E, X_E, X_D);
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_F, X_A, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_F, X_F, X_D);
            ctx.load_imm(X_B, 3);
            ctx.store_slot(X_B, dst_off);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.store_slot(X_E, dst_off + 16);
            ctx.store_slot(X_F, dst_off + 24);
            let done3 = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(not_3, SkipKind::Cond(Cond::Cs));
            ctx.push(
                encode::enc_lsr_imm(X_C, X_A, 18, true),
                format!("lsr {}, {}, #18", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_A],
            );
            ctx.load_imm(X_D, 0xF0);
            ctx.orr_reg(X_C, X_C, X_D);
            ctx.push(
                encode::enc_lsr_imm(X_E, X_A, 12, true),
                format!("lsr {}, {}, #12", reg_name(X_E), reg_name(X_A)),
                CostRule::Alu,
                Some(X_E),
                &[X_A],
            );
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_E, X_E, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_E, X_E, X_D);
            ctx.push(
                encode::enc_lsr_imm(X_F, X_A, 6, true),
                format!("lsr {}, {}, #6", reg_name(X_F), reg_name(X_A)),
                CostRule::Alu,
                Some(X_F),
                &[X_A],
            );
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_F, X_F, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_F, X_F, X_D);
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_B, X_A, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_B, X_B, X_D);
            if capacity < 4 {
                return Err(CodegenError::internal(
                    "FormatScalar char capacity < 4".to_string(),
                ));
            }
            ctx.load_imm(X_D, 4);
            ctx.store_slot(X_D, dst_off);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.store_slot(X_E, dst_off + 16);
            ctx.store_slot(X_F, dst_off + 24);
            ctx.store_slot(X_B, dst_off + 32);
            ctx.patch_skip(done3, SkipKind::Cond(Cond::Al));
            ctx.patch_skip(done2, SkipKind::Cond(Cond::Al));
            ctx.patch_skip(done, SkipKind::Cond(Cond::Al));
            Ok(())
        }
        Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize => {
            let signed = matches!(
                src_ty,
                Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Isize
            );
            if capacity == 0 {
                return Err(CodegenError::internal(
                    "FormatScalar integer capacity is 0".to_string(),
                ));
            }
            ctx.load_slot(X_A, src_off);
            ctx.load_imm(X_F, 0);
            if signed {
                ctx.push_flags(
                    encode::enc_cmp_reg(X_A, X_ZR, true),
                    format!("cmp {}, xzr", reg_name(X_A)),
                    CostRule::Alu,
                    None,
                    &[X_A, X_ZR],
                    FlagEffect::Write,
                );
                let nonneg = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
                ctx.load_imm(X_F, 1);
                ctx.push(
                    encode::enc_sub_reg(X_A, X_ZR, X_A, true),
                    format!("neg {}, {}", reg_name(X_A), reg_name(X_A)),
                    CostRule::Alu,
                    Some(X_A),
                    &[X_ZR, X_A],
                );
                ctx.patch_skip(nonneg, SkipKind::Cond(Cond::Ge));
            }
            let nonzero = ctx.emit_skip(SkipKind::Cbnz(X_A));
            ctx.load_imm(X_B, b'0' as i64);
            ctx.store_slot(X_B, dst_off + 8);
            ctx.load_imm(X_B, 1);
            ctx.push_flags(
                encode::enc_cmp_reg(X_F, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_F)),
                CostRule::Alu,
                None,
                &[X_F, X_ZR],
                FlagEffect::Write,
            );
            let no_sign0 = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
            ctx.load_imm(X_C, b'-' as i64);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.load_imm(X_C, b'0' as i64);
            ctx.store_slot(X_C, dst_off + 16);
            ctx.load_imm(X_B, 2);
            ctx.patch_skip(no_sign0, SkipKind::Cond(Cond::Eq));
            ctx.store_slot(X_B, dst_off);
            let done0 = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(nonzero, SkipKind::Cbnz(X_A));

            ctx.load_imm(X_I_REG, capacity as i64);
            ctx.load_imm(X_N_REG, 0);
            let loop_start = ctx.cur_word();
            ctx.load_imm(X_B, 10);
            ctx.push(
                encode::enc_udiv(X_C, X_A, X_B, true),
                format!(
                    "udiv {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
                CostRule::Udiv,
                Some(X_C),
                &[X_A, X_B],
            );
            ctx.push(
                encode::enc_msub(X_D, X_C, X_B, X_A, true),
                format!(
                    "msub {}, {}, {}, {}",
                    reg_name(X_D),
                    reg_name(X_C),
                    reg_name(X_B),
                    reg_name(X_A)
                ),
                CostRule::Mul,
                Some(X_D),
                &[X_C, X_B, X_A],
            );
            ctx.load_imm(X_B, b'0' as i64);
            ctx.add_reg(X_D, X_D, X_B);
            ctx.push(
                encode::enc_sub_imm(X_I_REG, X_I_REG, 1, true),
                format!("sub {}, {}, #1", reg_name(X_I_REG), reg_name(X_I_REG)),
                CostRule::Alu,
                Some(X_I_REG),
                &[X_I_REG],
            );
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.load_imm(X_B, 8);
            ctx.mul_reg(X_B, X_I_REG, X_B);
            ctx.add_reg(X_E, X_E, X_B);
            ctx.store_ptr(X_D, X_E, 0);
            ctx.push(
                encode::enc_add_imm(X_N_REG, X_N_REG, 1, true),
                format!("add {}, {}, #1", reg_name(X_N_REG), reg_name(X_N_REG)),
                CostRule::Alu,
                Some(X_N_REG),
                &[X_N_REG],
            );
            ctx.push(
                encode::enc_mov_reg(X_A, X_C, true),
                format!("mov {}, {}", reg_name(X_A), reg_name(X_C)),
                CostRule::Alu,
                Some(X_A),
                &[X_C],
            );
            let here = ctx.cur_word();
            let back = (loop_start as i64 - here as i64) as i32 * 4;
            ctx.push(
                encode::enc_cbnz(X_A, back, true),
                format!("cbnz {}, #{back}", reg_name(X_A)),
                CostRule::Branch,
                None,
                &[X_A],
            );

            ctx.push_flags(
                encode::enc_cmp_reg(X_F, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_F)),
                CostRule::Alu,
                None,
                &[X_F, X_ZR],
                FlagEffect::Write,
            );
            let no_sign = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
            ctx.push(
                encode::enc_sub_imm(X_I_REG, X_I_REG, 1, true),
                format!("sub {}, {}, #1", reg_name(X_I_REG), reg_name(X_I_REG)),
                CostRule::Alu,
                Some(X_I_REG),
                &[X_I_REG],
            );
            ctx.load_imm(X_D, b'-' as i64);
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.load_imm(X_B, 8);
            ctx.mul_reg(X_B, X_I_REG, X_B);
            ctx.add_reg(X_E, X_E, X_B);
            ctx.store_ptr(X_D, X_E, 0);
            ctx.push(
                encode::enc_add_imm(X_N_REG, X_N_REG, 1, true),
                format!("add {}, {}, #1", reg_name(X_N_REG), reg_name(X_N_REG)),
                CostRule::Alu,
                Some(X_N_REG),
                &[X_N_REG],
            );
            ctx.patch_skip(no_sign, SkipKind::Cond(Cond::Eq));

            ctx.load_imm(X_A, 0);
            let shift_start = ctx.cur_word();
            ctx.cmp_reg(X_A, X_N_REG);
            let shift_done = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            ctx.add_reg(X_B, X_I_REG, X_A);
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.load_imm(X_C, 8);
            ctx.mul_reg(X_D, X_B, X_C);
            ctx.add_reg(X_E, X_E, X_D);
            ctx.load_ptr(X_F, X_E, 0);
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.mul_reg(X_D, X_A, X_C);
            ctx.add_reg(X_E, X_E, X_D);
            ctx.store_ptr(X_F, X_E, 0);
            ctx.push(
                encode::enc_add_imm(X_A, X_A, 1, true),
                format!("add {}, {}, #1", reg_name(X_A), reg_name(X_A)),
                CostRule::Alu,
                Some(X_A),
                &[X_A],
            );
            let here = ctx.cur_word();
            let back = (shift_start as i64 - here as i64) as i32 * 4;
            ctx.push(
                encode::enc_b(back),
                format!("b #{back}"),
                CostRule::Branch,
                None,
                &[],
            );
            ctx.patch_skip(shift_done, SkipKind::Cond(Cond::Cs));

            for i in 0..capacity {
                ctx.load_imm(X_A, i as i64);
                ctx.cmp_reg(X_A, X_N_REG);
                let keep = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
                ctx.load_imm(X_B, 0);
                ctx.store_slot(X_B, dst_off + 8 * (1 + i));
                ctx.patch_skip(keep, SkipKind::Cond(Cond::Cc));
            }
            ctx.store_slot(X_N_REG, dst_off);
            ctx.patch_skip(done0, SkipKind::Cond(Cond::Al));
            Ok(())
        }
        other => Err(CodegenError::internal(format!(
            "FormatScalar for non-scalar type `{}`",
            crate::sema::types::render_type(other)
        ))),
    }
}

const X_I_REG: u8 = 15;
const X_N_REG: u8 = 16;

fn emit_string_concat(
    ctx: &mut FnCtx,
    dst: Temp,
    lhs: Temp,
    rhs: Temp,
    lhs_cap: usize,
    rhs_cap: usize,
) {
    let dst_off = ctx.frame.off(dst);
    let lhs_off = ctx.frame.off(lhs);
    let rhs_off = ctx.frame.off(rhs);
    let out_cap = lhs_cap + rhs_cap;
    for i in 0..=out_cap {
        ctx.load_imm(X_A, 0);
        ctx.store_slot(X_A, dst_off + 8 * i);
    }
    ctx.load_slot(X_A, lhs_off);
    ctx.load_slot(X_B, rhs_off);
    ctx.add_reg(X_C, X_A, X_B);
    ctx.store_slot(X_C, dst_off);
    for i in 0..lhs_cap {
        ctx.load_imm(X_D, i as i64);
        ctx.cmp_reg(X_D, X_A);
        let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
        ctx.load_slot(X_E, lhs_off + 8 * (1 + i));
        ctx.store_slot(X_E, dst_off + 8 * (1 + i));
        ctx.patch_skip(skip, SkipKind::Cond(Cond::Cs));
    }
    for j in 0..rhs_cap {
        ctx.load_imm(X_D, j as i64);
        ctx.cmp_reg(X_D, X_B);
        let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
        ctx.load_slot(X_E, rhs_off + 8 * (1 + j));
        ctx.addr_of_slot(X_F, dst_off + 8);
        ctx.add_reg(X_C, X_A, X_D);
        ctx.load_imm(X_D, 8);
        ctx.mul_reg(X_D, X_C, X_D);
        ctx.add_reg(X_F, X_F, X_D);
        ctx.store_ptr(X_E, X_F, 0);
        ctx.patch_skip(skip, SkipKind::Cond(Cond::Cs));
    }
}

fn emit_index_addr(
    ctx: &mut FnCtx,
    base_off: usize,
    index_off: usize,
    len: usize,
    elem_size: usize,
    out_reg: u8,
) {
    let x_a = ctx.use_slot(X_A, index_off);
    ctx.load_imm(X_B, len as i64);
    ctx.cmp_reg(x_a, X_B);
    let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val(
        "index ",
        x_a,
        false,
        &format!(" out of bounds (length {len})"),
    );
    ctx.patch_skip(skip, SkipKind::Cond(Cond::Cc));
    ctx.addr_of_slot(out_reg, base_off);
    ctx.load_imm(X_D, elem_size as i64);
    ctx.mul_reg(X_E, x_a, X_D);
    ctx.add_reg(out_reg, out_reg, X_E);
}

fn emit_bytes_index_addr(
    ctx: &mut FnCtx,
    handle_off: usize,
    index_off: usize,
    out_reg: u8,
) -> Result<(), CodegenError> {
    let x_a = ctx.use_slot(X_A, index_off);
    let x_b = ctx.use_slot(X_B, handle_off + 8);
    ctx.cmp_reg(x_a, x_b);
    let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val("index ", x_a, false, " out of bounds (Bytes)");
    ctx.patch_skip(skip, SkipKind::Cond(Cond::Cc));
    ctx.load_slot(out_reg, handle_off);
    ctx.add_reg(out_reg, out_reg, x_a);
    Ok(())
}

fn emit_placed_index_addr(
    ctx: &mut FnCtx,
    base_off: usize,
    field_offset: u64,
    index_off: usize,
    len: usize,
    elem_stride: u64,
    out_reg: u8,
) {
    let x_a = ctx.use_slot(X_A, index_off);
    ctx.load_imm(X_B, len as i64);
    ctx.cmp_reg(x_a, X_B);
    let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val(
        "index ",
        x_a,
        false,
        &format!(" out of bounds (length {len})"),
    );
    ctx.patch_skip(skip, SkipKind::Cond(Cond::Cc));
    ctx.load_slot(out_reg, base_off);
    if field_offset != 0 {
        ctx.load_imm(X_D, field_offset as i64);
        ctx.add_reg(out_reg, out_reg, X_D);
    }
    ctx.load_imm(X_D, elem_stride as i64);
    ctx.mul_reg(X_E, x_a, X_D);
    ctx.add_reg(out_reg, out_reg, X_E);
}

fn emit_arith_checked(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    dst: Temp,
    abort: &str,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::unimplemented("floating-point arithmetic"));
    }
    let (bits, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`ArithChecked` on non-integer {ty:?}")))?;
    let x_a = ctx.use_slot(X_A, ctx.frame.off(lhs));
    let x_b = ctx.use_slot(X_B, ctx.frame.off(rhs));
    let dst_off = ctx.frame.off(dst);
    let x_c = ctx.def_reg(X_C, dst_off);
    if bits < 64 {
        let (enc, mnem) = match op {
            BinOp::Add => (encode::enc_add_reg(x_c, x_a, x_b, true), "add"),
            BinOp::Sub => (encode::enc_sub_reg(x_c, x_a, x_b, true), "sub"),
            BinOp::Mul => (encode::enc_mul(x_c, x_a, x_b, true), "mul"),
            other => {
                return Err(CodegenError::internal(format!(
                    "`ArithChecked` with op `{}`",
                    other.as_str()
                )));
            }
        };
        ctx.push(
            enc,
            format!(
                "{mnem} {}, {}, {}",
                reg_name(x_c),
                reg_name(x_a),
                reg_name(x_b)
            ),
            match op {
                BinOp::Mul => CostRule::Mul,
                _ => CostRule::Alu,
            },
            Some(x_c),
            &[x_a, x_b],
        );
        ctx.check_int_range_or_abort(x_c, bits, signed, abort);
        ctx.store_slot(x_c, dst_off);
        return Ok(());
    }
    match op {
        BinOp::Add => {
            ctx.push_flags(
                encode::enc_adds_reg(x_c, x_a, x_b, true),
                format!(
                    "adds {}, {}, {}",
                    reg_name(x_c),
                    reg_name(x_a),
                    reg_name(x_b)
                ),
                CostRule::Alu,
                Some(x_c),
                &[x_a, x_b],
                FlagEffect::Write,
            );
            let fail = if signed { Cond::Vs } else { Cond::Cs };
            ctx.check_flags_or_abort(fail, abort);
        }
        BinOp::Sub => {
            ctx.push_flags(
                encode::enc_subs_reg(x_c, x_a, x_b, true),
                format!(
                    "subs {}, {}, {}",
                    reg_name(x_c),
                    reg_name(x_a),
                    reg_name(x_b)
                ),
                CostRule::Alu,
                Some(x_c),
                &[x_a, x_b],
                FlagEffect::Write,
            );
            let fail = if signed { Cond::Vs } else { Cond::Cc };
            ctx.check_flags_or_abort(fail, abort);
        }
        BinOp::Mul => {
            ctx.mul_reg(x_c, x_a, x_b);
            if signed {
                ctx.push(
                    encode::enc_smulh(X_D, x_a, x_b),
                    format!(
                        "smulh {}, {}, {}",
                        reg_name(X_D),
                        reg_name(x_a),
                        reg_name(x_b)
                    ),
                    CostRule::MulHigh,
                    Some(X_D),
                    &[x_a, x_b],
                );
                ctx.push(
                    encode::enc_asr_imm(X_E, x_c, 63, true),
                    format!("asr {}, {}, #63", reg_name(X_E), reg_name(x_c)),
                    CostRule::Alu,
                    Some(X_E),
                    &[x_c],
                );
                ctx.cmp_reg(X_D, X_E);
            } else {
                ctx.push(
                    encode::enc_umulh(X_D, x_a, x_b),
                    format!(
                        "umulh {}, {}, {}",
                        reg_name(X_D),
                        reg_name(x_a),
                        reg_name(x_b)
                    ),
                    CostRule::MulHigh,
                    Some(X_D),
                    &[x_a, x_b],
                );
                ctx.push_flags(
                    encode::enc_cmp_reg(X_D, X_ZR, true),
                    format!("cmp {}, xzr", reg_name(X_D)),
                    CostRule::Alu,
                    None,
                    &[X_D, X_ZR],
                    FlagEffect::Write,
                );
            }
            ctx.check_flags_or_abort(Cond::Ne, abort);
        }
        other => {
            return Err(CodegenError::internal(format!(
                "`ArithChecked` (64-bit) with op `{}`",
                other.as_str()
            )));
        }
    }
    ctx.store_slot(x_c, dst_off);
    Ok(())
}

fn emit_arith_wrapping(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    dst: Temp,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::unimplemented(
            "floating-point arithmetic (ArithWrapping doubles as float `+ - * / %`)",
        ));
    }
    let (bits, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`ArithWrapping` on non-integer {ty:?}")))?;
    let x_a = ctx.use_slot(X_A, ctx.frame.off(lhs));
    let x_b = ctx.use_slot(X_B, ctx.frame.off(rhs));
    let dst_off = ctx.frame.off(dst);
    let x_c = ctx.def_reg(X_C, dst_off);
    let (enc, mnem, rule) = match op {
        BinOp::AddW => (
            encode::enc_add_reg(x_c, x_a, x_b, true),
            "add",
            CostRule::Alu,
        ),
        BinOp::SubW => (
            encode::enc_sub_reg(x_c, x_a, x_b, true),
            "sub",
            CostRule::Alu,
        ),
        BinOp::MulW if bits <= 32 => {
            ctx.push(
                encode::enc_mul(x_c, x_a, x_b, false),
                format!("mul w{x_c}, w{x_a}, w{x_b}"),
                CostRule::MulW,
                None,
                &[],
            );
            ctx.narrow_to_width(x_c, bits, signed);
            ctx.store_slot(x_c, dst_off);
            return Ok(());
        }
        BinOp::MulW => (encode::enc_mul(x_c, x_a, x_b, true), "mul", CostRule::Mul),
        other => {
            return Err(CodegenError::internal(format!(
                "`ArithWrapping` with op `{}`",
                other.as_str()
            )));
        }
    };
    ctx.push(
        enc,
        format!(
            "{mnem} {}, {}, {}",
            reg_name(x_c),
            reg_name(x_a),
            reg_name(x_b)
        ),
        rule,
        None,
        &[],
    );
    ctx.narrow_to_width(x_c, bits, signed);
    ctx.store_slot(x_c, dst_off);
    Ok(())
}

fn emit_div_rem(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    dst: Temp,
    abort_zero: &str,
    abort_overflow: &str,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::unimplemented("floating-point division"));
    }
    let (_, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`DivRem` on non-integer {ty:?}")))?;
    let x_a = ctx.use_slot(X_A, ctx.frame.off(lhs));
    let x_b = ctx.use_slot(X_B, ctx.frame.off(rhs));
    let dst_off = ctx.frame.off(dst);
    let x_c = ctx.def_reg(X_C, dst_off);
    ctx.push_flags(
        encode::enc_cmp_reg(x_b, X_ZR, true),
        format!("cmp {}, xzr", reg_name(x_b)),
        CostRule::Alu,
        None,
        &[x_b, X_ZR],
        FlagEffect::Write,
    );
    let skip_zero = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
    ctx.abort_fixed(abort_zero);
    ctx.patch_skip(skip_zero, SkipKind::Cond(Cond::Ne));
    if signed && op == BinOp::Div {
        let (min, _) = int_bounds_i64(ty).unwrap();
        ctx.load_imm(X_D, min);
        ctx.cmp_reg(x_a, X_D);
        let skip_a = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
        ctx.load_imm(X_E, -1);
        ctx.cmp_reg(x_b, X_E);
        let skip_b = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
        ctx.abort_fixed(abort_overflow);
        ctx.patch_skip(skip_a, SkipKind::Cond(Cond::Ne));
        ctx.patch_skip(skip_b, SkipKind::Cond(Cond::Ne));
    }
    let (enc, mnem, rule) = if signed {
        (
            encode::enc_sdiv(x_c, x_a, x_b, true),
            "sdiv",
            CostRule::Sdiv,
        )
    } else {
        (
            encode::enc_udiv(x_c, x_a, x_b, true),
            "udiv",
            CostRule::Udiv,
        )
    };
    ctx.push(
        enc,
        format!(
            "{mnem} {}, {}, {}",
            reg_name(x_c),
            reg_name(x_a),
            reg_name(x_b)
        ),
        rule,
        Some(x_c),
        &[x_a, x_b],
    );
    if op == BinOp::Rem {
        ctx.push(
            encode::enc_msub(x_c, x_c, x_b, x_a, true),
            format!(
                "msub {}, {}, {}, {}",
                reg_name(x_c),
                reg_name(x_c),
                reg_name(x_b),
                reg_name(x_a)
            ),
            CostRule::Mul,
            Some(x_c),
            &[x_c, x_b, x_a],
        );
    } else if op != BinOp::Div {
        return Err(CodegenError::internal(format!(
            "`DivRem` with op `{}`",
            op.as_str()
        )));
    }
    ctx.store_slot(x_c, dst_off);
    Ok(())
}

fn emit_shift(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    bits: u32,
    lost: Option<&str>,
    dst: Temp,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::internal("`Shift` with a float type"));
    }
    let (_, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`Shift` on non-integer {ty:?}")))?;
    let x_a = ctx.use_slot(X_A, ctx.frame.off(lhs));
    let x_b = ctx.use_slot(X_B, ctx.frame.off(rhs));
    let dst_off = ctx.frame.off(dst);
    let x_f = ctx.def_reg(X_F, dst_off);
    ctx.load_imm(X_D, bits as i64);
    ctx.cmp_reg(x_b, X_D);
    let skip_range = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val(
        "shift count ",
        x_b,
        signed,
        &format!(" is out of range for a {bits}-bit type"),
    );
    ctx.patch_skip(skip_range, SkipKind::Cond(Cond::Cc));

    if op == BinOp::Shl {
        let skip_zero = ctx.emit_skip(SkipKind::Cbz(x_b));
        ctx.push(
            encode::enc_mov_reg(X_C, x_a, true),
            format!("mov {}, {}", reg_name(X_C), reg_name(x_a)),
            CostRule::Alu,
            Some(X_C),
            &[x_a],
        );
        ctx.narrow_to_width(X_C, bits, false);
        ctx.load_imm(X_D, bits as i64);
        ctx.push(
            encode::enc_sub_reg(X_D, X_D, x_b, true),
            format!(
                "sub {}, {}, {}",
                reg_name(X_D),
                reg_name(X_D),
                reg_name(x_b)
            ),
            CostRule::Alu,
            Some(X_D),
            &[X_D, x_b],
        );
        ctx.push(
            encode::enc_lsr_reg(X_E, X_C, X_D, true),
            format!(
                "lsr {}, {}, {}",
                reg_name(X_E),
                reg_name(X_C),
                reg_name(X_D)
            ),
            CostRule::Alu,
            Some(X_E),
            &[X_C, X_D],
        );
        ctx.push_flags(
            encode::enc_cmp_reg(X_E, X_ZR, true),
            format!("cmp {}, xzr", reg_name(X_E)),
            CostRule::Alu,
            None,
            &[X_E, X_ZR],
            FlagEffect::Write,
        );
        let skip_lost = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
        let lost_msg = lost.ok_or_else(|| {
            CodegenError::internal("`Shift` Shl with no `lost` message (mwir producer bug)")
        })?;
        ctx.abort_fixed(lost_msg);
        ctx.patch_skip(skip_lost, SkipKind::Cond(Cond::Eq));
        ctx.patch_skip(skip_zero, SkipKind::Cbz(x_b));

        ctx.push(
            encode::enc_lsl_reg(x_f, x_a, x_b, true),
            format!(
                "lsl {}, {}, {}",
                reg_name(x_f),
                reg_name(x_a),
                reg_name(x_b)
            ),
            CostRule::Alu,
            Some(x_f),
            &[x_a, x_b],
        );
        ctx.narrow_to_width(x_f, bits, signed);
        ctx.store_slot(x_f, dst_off);
    } else if op == BinOp::Shr {
        let (enc, mnem) = if signed {
            (encode::enc_asr_reg(x_f, x_a, x_b, true), "asr")
        } else {
            (encode::enc_lsr_reg(x_f, x_a, x_b, true), "lsr")
        };
        ctx.push(
            enc,
            format!(
                "{mnem} {}, {}, {}",
                reg_name(x_f),
                reg_name(x_a),
                reg_name(x_b)
            ),
            CostRule::Alu,
            None,
            &[],
        );
        ctx.store_slot(x_f, dst_off);
    } else {
        return Err(CodegenError::internal(format!(
            "`Shift` with op `{}`",
            op.as_str()
        )));
    }
    Ok(())
}

fn emit_convert(
    ctx: &mut FnCtx,
    f: &MwirFn,
    target_ty: &Type,
    src: Temp,
    dst: Temp,
    abort: &str,
) -> Result<(), CodegenError> {
    let src_ty = f.temp_types[src.0].clone();
    if is_float(target_ty) || is_float(&src_ty) {
        return Err(CodegenError::unimplemented(
            "floating-point `.to[T]()` conversion",
        ));
    }
    let (tbits, tsigned) = int_shape(target_ty)
        .ok_or_else(|| CodegenError::internal(format!("`Convert` target {target_ty:?}")))?;
    let (sbits, ssigned) = int_shape(&src_ty)
        .ok_or_else(|| CodegenError::internal(format!("`Convert` source {src_ty:?}")))?;
    let x_a = ctx.use_slot(X_A, ctx.frame.off(src));
    let dst_off = ctx.frame.off(dst);
    let x_c = ctx.def_reg(X_C, dst_off);
    if tbits == 64 && !tsigned {
        if ssigned {
            ctx.push_flags(
                encode::enc_cmp_reg(x_a, X_ZR, true),
                format!("cmp {}, xzr", reg_name(x_a)),
                CostRule::Alu,
                None,
                &[x_a, X_ZR],
                FlagEffect::Write,
            );
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ge));
        }
    } else if tbits == 64 && tsigned {
        if !ssigned && sbits == 64 {
            ctx.push_flags(
                encode::enc_cmp_reg(x_a, X_ZR, true),
                format!("cmp {}, xzr", reg_name(x_a)),
                CostRule::Alu,
                None,
                &[x_a, X_ZR],
                FlagEffect::Write,
            );
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ge));
        }
    } else {
        ctx.check_int_range_or_abort(x_a, tbits, tsigned, abort);
    }
    ctx.push(
        encode::enc_mov_reg(x_c, x_a, true),
        format!("mov {}, {}", reg_name(x_c), reg_name(x_a)),
        CostRule::Alu,
        Some(x_c),
        &[x_a],
    );
    ctx.narrow_to_width(x_c, tbits, tsigned);
    ctx.store_slot(x_c, dst_off);
    Ok(())
}

fn emit_prologue(f: &MwirFn, frame: &Frame, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if !frame.frameless {
        ctx.push(
            encode::enc_sub_imm(X_SP, X_SP, frame.size as u16, true),
            format!("sub sp, sp, #{}", frame.size),
            CostRule::Alu,
            Some(X_SP),
            &[X_SP],
        );
    }
    if frame.lr_saved {
        ctx.store_slot(X_LR, frame.lr_off);
    }
    let mut next_reg = 0u8;
    if let Some((self_temp, mode)) = f.receiver {
        let self_ty = &f.temp_types[self_temp.0];
        if is_aggregate(self_ty) || mode == AccessMode::Mut {
            let self_ptr_off = frame
                .self_ptr_off
                .ok_or_else(|| CodegenError::internal("receiver present but no self_ptr slot"))?;
            ctx.store_slot(next_reg, self_ptr_off);
            copy_self_fields_skipping_interrupt_cells(
                f,
                frame,
                self_temp,
                ctx,
                SelfFieldCopy::LiveToFrame,
            )?;
        } else {
            ctx.store_slot(next_reg, frame.off(self_temp));
        }
        next_reg += 1;
    }
    let mut mut_ptr_iter = frame.mut_param_ptr_offs.iter();
    for (p, mode) in &f.params {
        if next_reg > 8 {
            return Err(CodegenError::unimplemented("more than 8 call arguments"));
        }
        let ty = &f.temp_types[p.0];
        if is_aggregate(ty) || *mode == AccessMode::Mut {
            if *mode == AccessMode::Mut {
                let (pt, ptr_off) = mut_ptr_iter.next().ok_or_else(|| {
                    CodegenError::internal("mut param missing from frame.mut_param_ptr_offs")
                })?;
                if *pt != *p {
                    return Err(CodegenError::internal(
                        "mut_param_ptr_offs order disagrees with MwirFn::params",
                    ));
                }
                ctx.store_slot(next_reg, *ptr_off);
            }
            let size = frame.size_of_temp(*p);
            let dst_off = frame.off(*p);
            let mut w = 0;
            while w < size {
                let d = ctx.def_reg(X_A, dst_off + w);
                ctx.load_ptr(d, next_reg, w);
                ctx.store_slot(d, dst_off + w);
                w += 8;
            }
        } else {
            ctx.store_slot(next_reg, frame.off(*p));
        }
        next_reg += 1;
    }
    if mut_ptr_iter.next().is_some() {
        return Err(CodegenError::internal(
            "frame.mut_param_ptr_offs has more entries than Mut params",
        ));
    }
    if let Some(ret_ptr_off) = frame.ret_ptr_off {
        ctx.store_slot(8, ret_ptr_off);
    }
    Ok(())
}

fn emit_epilogue(f: &MwirFn, frame: &Frame, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if let Some((self_temp, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            copy_self_fields_skipping_interrupt_cells(
                f,
                frame,
                self_temp,
                ctx,
                SelfFieldCopy::FrameToLive,
            )?;
        }
    }
    for (p, ptr_off) in &frame.mut_param_ptr_offs {
        let base = ctx.use_slot(X_A, *ptr_off);
        let size = frame.size_of_temp(*p);
        let src_off = frame.off(*p);
        let mut w = 0;
        while w < size {
            let v = ctx.use_slot(X_B, src_off + w);
            ctx.store_ptr(v, base, w);
            w += 8;
        }
    }
    emit_frame_teardown(frame, ctx);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
    Ok(())
}

fn emit_frame_teardown(frame: &Frame, ctx: &mut FnCtx) {
    if frame.lr_saved {
        ctx.load_slot(X_LR, frame.lr_off);
    }
    if frame.frameless {
        return;
    }
    ctx.push(
        encode::enc_add_imm(X_SP, X_SP, frame.size as u16, true),
        format!("add sp, sp, #{}", frame.size),
        CostRule::Alu,
        Some(X_SP),
        &[X_SP],
    );
}

enum InterruptCellRmw {
    Swap,
    FetchOr,
}

fn interrupt_cell_memref(field_off: usize) -> MemRef {
    MemRef::for_base_imm(X_A, field_off as u64)
}

fn emit_interrupt_cell_addr(ctx: &mut FnCtx, field_off: usize) -> Result<(), CodegenError> {
    let self_ptr_off = ctx.frame.self_ptr_off.ok_or_else(|| {
        CodegenError::internal("InterruptCell op needs a receiver (self_ptr slot)")
    })?;
    ctx.load_slot(X_A, self_ptr_off);
    if field_off != 0 {
        if field_off > 4095 {
            return Err(CodegenError::unimplemented(
                "InterruptCell field_off above add-immediate range",
            ));
        }
        ctx.push(
            encode::enc_add_imm(X_A, X_A, field_off as u16, true),
            format!("add {}, {}, #{field_off}", reg_name(X_A), reg_name(X_A)),
            CostRule::Alu,
            Some(X_A),
            &[X_A],
        );
    }
    Ok(())
}

fn emit_interrupt_cell_rmw(
    ctx: &mut FnCtx,
    field_off: usize,
    width: u8,
    value_off: usize,
    kind: InterruptCellRmw,
) -> Result<(), CodegenError> {
    emit_interrupt_cell_addr(ctx, field_off)?;
    ctx.load_slot(X_B, value_off);
    let mem = Some(interrupt_cell_memref(field_off));
    match width {
        4 => {
            ctx.push_mem(
                encode::enc_ldar_w(X_C, X_A),
                format!("ldar w{}, [{}]", X_C, reg_name(X_A)),
                CostRule::LoadAcquire,
                Some(X_C),
                &[X_A],
                mem,
            );
            match kind {
                InterruptCellRmw::Swap => {
                    ctx.push_mem(
                        encode::enc_stlr_w(X_B, X_A),
                        format!("stlr w{}, [{}]", X_B, reg_name(X_A)),
                        CostRule::StoreRelease,
                        None,
                        &[X_A, X_B],
                        mem,
                    );
                }
                InterruptCellRmw::FetchOr => {
                    ctx.push(
                        encode::enc_orr_reg(X_D, X_C, X_B, false),
                        format!("orr w{}, w{}, w{}", X_D, X_C, X_B),
                        CostRule::Alu,
                        Some(X_D),
                        &[X_C, X_B],
                    );
                    ctx.push_mem(
                        encode::enc_stlr_w(X_D, X_A),
                        format!("stlr w{}, [{}]", X_D, reg_name(X_A)),
                        CostRule::StoreRelease,
                        None,
                        &[X_A, X_D],
                        mem,
                    );
                }
            }
        }
        8 => {
            ctx.push_mem(
                encode::enc_ldar_x(X_C, X_A),
                format!("ldar {}, [{}]", reg_name(X_C), reg_name(X_A)),
                CostRule::LoadAcquire,
                Some(X_C),
                &[X_A],
                mem,
            );
            match kind {
                InterruptCellRmw::Swap => {
                    ctx.push_mem(
                        encode::enc_stlr_x(X_B, X_A),
                        format!("stlr {}, [{}]", reg_name(X_B), reg_name(X_A)),
                        CostRule::StoreRelease,
                        None,
                        &[X_A, X_B],
                        mem,
                    );
                }
                InterruptCellRmw::FetchOr => {
                    ctx.orr_reg(X_D, X_C, X_B);
                    ctx.push_mem(
                        encode::enc_stlr_x(X_D, X_A),
                        format!("stlr {}, [{}]", reg_name(X_D), reg_name(X_A)),
                        CostRule::StoreRelease,
                        None,
                        &[X_A, X_D],
                        mem,
                    );
                }
            }
        }
        w => {
            return Err(CodegenError::internal(format!(
                "InterruptCell RMW width {w}"
            )));
        }
    }
    Ok(())
}

enum SelfFieldCopy {
    LiveToFrame,
    FrameToLive,
}

fn copy_self_fields_skipping_interrupt_cells(
    f: &MwirFn,
    frame: &Frame,
    self_temp: Temp,
    ctx: &mut FnCtx,
    dir: SelfFieldCopy,
) -> Result<(), CodegenError> {
    let self_ptr_off = frame
        .self_ptr_off
        .ok_or_else(|| CodegenError::internal("mut receiver but no self_ptr slot"))?;
    ctx.load_slot(X_A, self_ptr_off);
    let self_ty = &f.temp_types[self_temp.0];
    let Type::Named(name, targs) = strip_wrappers(self_ty) else {
        copy_self_aggregate_words(frame, self_temp, ctx, dir)?;
        return Ok(());
    };
    let layout_key = if targs.is_empty() {
        name.clone()
    } else {
        crate::sema::types::render_type(&Type::Named(name.clone(), targs.to_vec()))
    };
    if ctx.layout.enums.contains_key(name.as_str()) || ctx.layout.enums.contains_key(&layout_key) {
        copy_self_aggregate_words(frame, self_temp, ctx, dir)?;
        return Ok(());
    }
    let fields = ctx.layout.structs.get(&layout_key).ok_or_else(|| {
        CodegenError::internal(format!("unknown struct `{layout_key}` in layout ctx"))
    })?;
    let frame_base = frame.off(self_temp);
    let mut off = 0usize;
    for field_ty in fields {
        let sz =
            mwir::size_of(field_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
        if !matches!(
            strip_wrappers(field_ty),
            Type::Named(n, _) if n == "InterruptCell"
        ) {
            let mut w = 0;
            while w < sz {
                match dir {
                    SelfFieldCopy::LiveToFrame => {
                        ctx.load_ptr(X_B, X_A, off + w);
                        ctx.store_slot(X_B, frame_base + off + w);
                    }
                    SelfFieldCopy::FrameToLive => {
                        ctx.load_slot(X_B, frame_base + off + w);
                        ctx.store_ptr(X_B, X_A, off + w);
                    }
                }
                w += 8;
            }
        }
        off += sz;
    }
    Ok(())
}

fn copy_self_aggregate_words(
    frame: &Frame,
    self_temp: Temp,
    ctx: &mut FnCtx,
    dir: SelfFieldCopy,
) -> Result<(), CodegenError> {
    let size = frame.size_of_temp(self_temp);
    let frame_off = frame.off(self_temp);
    let mut w = 0;
    while w < size {
        match dir {
            SelfFieldCopy::LiveToFrame => {
                ctx.load_ptr(X_B, X_A, w);
                ctx.store_slot(X_B, frame_off + w);
            }
            SelfFieldCopy::FrameToLive => {
                ctx.load_slot(X_B, frame_off + w);
                ctx.store_ptr(X_B, X_A, w);
            }
        }
        w += 8;
    }
    Ok(())
}

fn emit_slotmap_mint_id(map: Temp, ctx: &mut FnCtx<'_>) -> Result<(), CodegenError> {
    let addr =
        wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_SLOTMAP_NEXT_ID;
    ctx.load_imm(X_A, addr as i64);
    ctx.push(
        encode::enc_ldr_x_imm(X_B, X_A, 0),
        format!(
            "ldr {}, [{}]  ; SlotMap next id",
            reg_name(X_B),
            reg_name(X_A)
        ),
        CostRule::Load,
        Some(X_B),
        &[X_A],
    );
    ctx.push(
        encode::enc_add_imm(X_C, X_B, 1, true),
        format!("add {}, {}, #1", reg_name(X_C), reg_name(X_B)),
        CostRule::Alu,
        Some(X_C),
        &[X_B],
    );
    let skip = ctx.emit_skip(SkipKind::Cbnz(X_C));
    ctx.abort_fixed(
        "SlotMap instance id space exhausted (u64 non-wrapping mint, 05-library.md §7)",
    );
    ctx.patch_skip(skip, SkipKind::Cbnz(X_C));
    ctx.push(
        encode::enc_str_x_imm(X_C, X_A, 0),
        format!("str {}, [{}]", reg_name(X_C), reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_C, X_A],
    );
    ctx.store_slot(X_C, ctx.frame.off(map));
    Ok(())
}

fn probe_fn_facts(
    f: &MwirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
    frame: &Frame,
    block_ids: &[Option<u32>],
) -> Result<regalloc::FnFacts, CodegenError> {
    let plan = &TailPlan::none(f.body.len());
    let no_elision = vec![false; f.body.len()];
    let dummy_targets = vec![0usize; f.body.len() + 1];
    let mut points: Vec<regalloc::PointFacts> = Vec::with_capacity(f.body.len() + 2);

    let finish = |ctx: FnCtx| -> regalloc::PointFacts {
        let mut touches = Vec::new();
        for &(off, how, word, reg) in &ctx.slot_accesses {
            if let Some((temp, is_base)) = frame.temp_at_offset(off) {
                let whole_slot = frame.temp_size[temp] == FRAME_SLOT_BYTES as usize;
                let how = if how == regalloc::Touch::Escape || !is_base || !whole_slot {
                    regalloc::Touch::Escape
                } else {
                    how
                };
                touches.push((temp, how, word, reg));
            }
        }
        let mut call_words = Vec::new();
        let mut regs = BTreeSet::new();
        let mut word_regs = Vec::new();
        for (i, w) in ctx.words.iter().enumerate() {
            if w.rule == CostRule::Call {
                let callee = ctx.relocs.iter().find_map(|r| match r {
                    Reloc::Call { word, key } if *word == i => Some(key.clone()),
                    _ => None,
                });
                call_words.push(regalloc::CallWord { word: i, callee });
            }
            if let Some(d) = w.dst {
                regs.insert(d);
                word_regs.push((i, d));
            }
            for &s in &w.srcs[..w.src_len as usize] {
                regs.insert(s);
                word_regs.push((i, s));
            }
        }
        regalloc::PointFacts {
            touches,
            call_words,
            regs,
            word_regs,
        }
    };

    {
        let mut ctx = FnCtx {
            frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_SP,
            slot_bias: 0,
            cold_seq: 0,
            slot_accesses: Vec::new(),
            resident_misuse: None,
            home_mask: frame.home_mask(),
            home_def_ok: None,
            elide_branch: false,
        };
        emit_prologue(f, frame, &mut ctx)?;
        points.push(finish(ctx));
    }

    for i in 0..f.body.len() {
        let mut ctx = FnCtx {
            frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_SP,
            slot_bias: 0,
            cold_seq: 0,
            slot_accesses: Vec::new(),
            resident_misuse: None,
            home_mask: frame.home_mask(),
            home_def_ok: None,
            elide_branch: false,
        };
        emit_body_inst(i, f, &mut ctx, plan, block_ids, &no_elision)?;
        points.push(finish(ctx));
    }

    {
        let mut ctx = FnCtx {
            frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_SP,
            slot_bias: 0,
            cold_seq: 0,
            slot_accesses: Vec::new(),
            resident_misuse: None,
            home_mask: frame.home_mask(),
            home_def_ok: None,
            elide_branch: false,
        };
        emit_epilogue(f, frame, &mut ctx)?;
        points.push(finish(ctx));
    }

    let mut back_edges = Vec::new();
    for (i, inst) in f.body.iter().enumerate() {
        let target = match inst {
            Inst::Jump { target } => Some(*target),
            Inst::JumpIfFalse { target, .. } => Some(*target),
            _ => None,
        };
        if let Some(t) = target {
            if t <= i {
                back_edges.push((i + 1, t + 1));
            }
        }
    }

    let mut calls: BTreeSet<String> = BTreeSet::new();
    let mut has_returning_call = false;
    for p in &points {
        if !p.call_words.is_empty() {
            has_returning_call = true;
        }
    }
    let opaque_calls = points
        .iter()
        .any(|p| p.call_words.iter().any(|c| c.callee.is_none()));
    for p in &points {
        for cw in &p.call_words {
            if let Some(k) = &cw.callee {
                calls.insert(k.clone());
            }
        }
    }
    Ok(regalloc::FnFacts {
        temp_count: f.temp_types.len(),
        points,
        back_edges,
        calls,
        opaque_calls,
        has_returning_call,
    })
}

#[derive(Clone, Debug, Default)]
struct TailPlan {
    at: Vec<Option<String>>,
    suppressed: Vec<bool>,
}

impl TailPlan {
    fn none(n: usize) -> TailPlan {
        TailPlan {
            at: vec![None; n],
            suppressed: vec![false; n],
        }
    }
}

fn plan_tail_calls(f: &MwirFn, block_ids: &[Option<u32>]) -> TailPlan {
    let n = f.body.len();
    let mut plan = TailPlan::none(n);
    if !tail_calls() {
        return plan;
    }
    if let Some((_, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            return plan;
        }
    }
    if f.params.iter().any(|(_, m)| *m == AccessMode::Mut) {
        return plan;
    }
    if is_aggregate(&f.ret) {
        return plan;
    }
    let mut branch_targets: BTreeSet<usize> = BTreeSet::new();
    for inst in &f.body {
        match inst {
            Inst::Jump { target } => {
                branch_targets.insert(*target);
            }
            Inst::JumpIfFalse { target, .. } => {
                branch_targets.insert(*target);
            }
            _ => {}
        }
    }
    for i in 0..n {
        let Inst::Call {
            dst,
            write_backs,
            key,
            args,
        } = &f.body[i]
        else {
            continue;
        };
        if !write_backs.is_empty() || args.len() > 8 {
            continue;
        }
        if args.iter().any(|a| is_aggregate(&f.temp_types[a.0])) {
            continue;
        }
        if is_aggregate(&f.temp_types[dst.0]) {
            continue;
        }
        if is_compiler_glue_symbol(key) || rt_enqueue_actor(key).is_some() {
            continue;
        }
        if i + 1 >= n {
            continue;
        }
        let returns_this = match &f.body[i + 1] {
            Inst::Return { value: Some(v) } => *v == *dst,
            Inst::Return { value: None } => true,
            _ => false,
        };
        if !returns_this {
            continue;
        }
        if branch_targets.contains(&(i + 1)) || block_ids[i + 1].is_some() {
            continue;
        }
        plan.at[i] = Some(key.clone());
        plan.suppressed[i + 1] = true;
    }
    plan
}

fn emit_tail_call(key: &str, args: &[Temp], ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if args.len() > 8 {
        return Err(CodegenError::unimplemented("more than 8 call arguments"));
    }
    for (i, arg) in args.iter().enumerate() {
        ctx.load_slot(i as u8, ctx.frame.off(*arg));
    }
    emit_frame_teardown(ctx.frame, ctx);
    let srcs: Vec<u8> = (0..args.len()).map(|i| i as u8).collect();
    let word = ctx.cur_word();
    ctx.push(
        encode::enc_b(0),
        format!("b <{key}>  ; tail call"),
        CostRule::Branch,
        None,
        &srcs,
    );
    ctx.relocs.push(Reloc::Call {
        word,
        key: key.to_string(),
    });
    Ok(())
}

fn emit_body_inst(
    i: usize,
    f: &MwirFn,
    ctx: &mut FnCtx,
    plan: &TailPlan,
    block_ids: &[Option<u32>],
    elide: &[bool],
) -> Result<(), CodegenError> {
    if plan.suppressed[i] {
        return Ok(());
    }
    if let Some(id) = block_ids[i] {
        if block_count() {
            ctx.emit_block_hit(id);
        }
    }
    if let Some(key) = &plan.at[i] {
        let Inst::Call { args, .. } = &f.body[i] else {
            return Err(CodegenError::internal(
                "tail-call plan names an instruction that is not a Call",
            ));
        };
        return emit_tail_call(key, args, ctx);
    }
    ctx.elide_branch = elide[i];
    let r = emit_one(&f.body[i], f, ctx);
    ctx.elide_branch = false;
    r
}

struct PreparedFn {
    block_ids: Vec<Option<u32>>,
    plan: TailPlan,
    assign: regalloc::Assignment,
    input: Option<regalloc::FnInput>,
    has_returning_call: Option<bool>,
}

fn prepare_fn(
    key: &str,
    f: &MwirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
) -> Result<PreparedFn, CodegenError> {
    let naive = regalloc::Assignment::none(f.temp_types.len());
    let frame = build_frame(f, layout, 0, mwir_entropy_scratch_size(f), 0, &naive, true)?;

    let block_ids = if block_count_instruments(key) {
        assign_mwir_block_ids(&f.body)?
    } else {
        vec![None; f.body.len()]
    };
    let plan = plan_tail_calls(f, &block_ids);

    if !regalloc::regalloc() {
        return Ok(PreparedFn {
            block_ids,
            plan,
            assign: naive,
            input: None,
            has_returning_call: None,
        });
    }

    let facts = probe_fn_facts(f, layout, rodata, &frame, &block_ids)?;
    let scalar_slot: Vec<bool> = frame
        .temp_size
        .iter()
        .map(|&s| s == FRAME_SLOT_BYTES as usize)
        .collect();
    let has_returning_call = facts.opaque_calls
        || facts.points.iter().enumerate().any(|(p, pf)| {
            !pf.call_words.is_empty()
                && match p.checked_sub(1) {
                    Some(i) if i < plan.at.len() => plan.at[i].is_none(),
                    _ => true,
                }
        });
    let (assign, input) = if regalloc::interproc_regs() {
        (
            regalloc::Assignment::none(f.temp_types.len()),
            Some(regalloc::FnInput {
                facts,
                scalar_slot,
                opaque_body: is_compiler_glue_symbol(key) || key.starts_with("__"),
            }),
        )
    } else {
        (regalloc::allocate(&facts, &scalar_slot), None)
    };
    Ok(PreparedFn {
        block_ids,
        plan,
        assign,
        input,
        has_returning_call: Some(has_returning_call),
    })
}

fn prepare_sync_fns(
    mwir: &MwirProgram,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
) -> Result<
    (
        BTreeMap<String, PreparedFn>,
        BTreeMap<String, regalloc::Convention>,
    ),
    CodegenError,
> {
    let mut prepared: BTreeMap<String, PreparedFn> = BTreeMap::new();
    for (key, f) in &mwir.fns {
        prepared.insert(key.clone(), prepare_fn(key, f, layout, rodata)?);
    }
    let inputs: BTreeMap<String, regalloc::FnInput> = prepared
        .iter()
        .filter_map(|(k, p)| p.input.as_ref().map(|i| (k.clone(), i.clone())))
        .collect();
    let conventions = if inputs.is_empty() {
        BTreeMap::new()
    } else {
        regalloc::allocate_program(&inputs)
    };
    Ok((prepared, conventions))
}

fn emit_fn(
    key: &str,
    f: &MwirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
    prepared: &PreparedFn,
    convention: Option<&regalloc::Convention>,
) -> Result<CodegenFn, CodegenError> {
    let block_ids = &prepared.block_ids;
    let assign = match convention {
        Some(c) => &c.assignment,
        None => &prepared.assign,
    };
    let save_lr = !frameless_fns() || prepared.has_returning_call != Some(false);
    let frame = build_frame(
        f,
        layout,
        0,
        mwir_entropy_scratch_size(f),
        0,
        assign,
        save_lr,
    )?;

    let no_tails = TailPlan::none(f.body.len());
    let plan: &TailPlan = if frame.lr_saved {
        &no_tails
    } else {
        &prepared.plan
    };

    let elide = sync_branch_elision(&f.body);

    let empty: [usize; 0] = [];
    let mut probe_pro = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &empty,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_SP,
        slot_bias: 0,
        cold_seq: 0,
        slot_accesses: Vec::new(),
        resident_misuse: None,
        home_mask: frame.home_mask(),
        home_def_ok: None,
        elide_branch: false,
    };
    emit_prologue(f, &frame, &mut probe_pro)?;
    let prologue_len = probe_pro.words.len();

    let dummy_targets = vec![0usize; f.body.len() + 1];
    let mut counts = Vec::with_capacity(f.body.len());
    for i in 0..f.body.len() {
        let mut probe = FnCtx {
            frame: &frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_SP,
            slot_bias: 0,
            cold_seq: 0,
            slot_accesses: Vec::new(),
            resident_misuse: None,
            home_mask: frame.home_mask(),
            home_def_ok: None,
            elide_branch: false,
        };
        emit_body_inst(i, f, &mut probe, plan, block_ids, &elide)?;
        counts.push(probe.words.len());
    }
    let mut word_offsets = vec![0usize; f.body.len() + 1];
    let mut acc = prologue_len;
    for (i, c) in counts.iter().enumerate() {
        word_offsets[i] = acc;
        acc += c;
    }
    word_offsets[f.body.len()] = acc;

    let mut ctx = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &word_offsets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_SP,
        slot_bias: 0,
        cold_seq: 0,
        slot_accesses: Vec::new(),
        resident_misuse: None,
        home_mask: frame.home_mask(),
        home_def_ok: None,
        elide_branch: false,
    };
    emit_prologue(f, &frame, &mut ctx)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for i in 0..f.body.len() {
        emit_body_inst(i, f, &mut ctx, plan, block_ids, &elide)?;
    }
    debug_assert_eq!(ctx.words.len(), word_offsets[f.body.len()]);
    emit_epilogue(f, &frame, &mut ctx)?;

    if let Some(what) = ctx.resident_misuse.take() {
        return Err(CodegenError::internal(what));
    }

    if block_bridge() {
        record_spans(key, block_ids, &word_offsets, ctx.words.len());
    }

    Ok(CodegenFn {
        frame_size: frame.size,
        code: ctx.words,
        relocs: ctx.relocs,
    })
}

pub(crate) fn mwir_block_leaders(body: &[Inst]) -> Vec<bool> {
    let n = body.len();
    let mut leaders = vec![false; n];
    if n == 0 {
        return leaders;
    }
    leaders[0] = true;
    for (i, inst) in body.iter().enumerate() {
        match inst {
            Inst::Jump { target } => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            Inst::JumpIfFalse { target, .. } => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            Inst::Return { .. } => {
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            _ => {}
        }
    }
    leaders
}

fn assign_mwir_block_ids(body: &[Inst]) -> Result<Vec<Option<u32>>, CodegenError> {
    let mut ids = vec![None; body.len()];
    if !block_ids_active() {
        return Ok(ids);
    }
    for (i, is_leader) in mwir_block_leaders(body).into_iter().enumerate() {
        if is_leader {
            ids[i] = Some(alloc_block_id()?);
        }
    }
    Ok(ids)
}

use crate::flowwir::{AwaitKind, FlowInst, FlowWirFn, FlowWirProgram, Transition};

pub fn rt_enqueue_symbol(actor: &str) -> String {
    format!("{RT_ENQUEUE_PREFIX}{actor}")
}

pub fn rt_enqueue_actor(key: &str) -> Option<&str> {
    key.strip_prefix(RT_ENQUEUE_PREFIX)
}

pub fn symbol_is_synthetic(key: &str) -> bool {
    key.contains(' ')
}

pub fn is_compiler_glue_symbol(key: &str) -> bool {
    symbol_is_synthetic(key)
        || key.starts_with("__wrela_")
        || key.starts_with("__enqueue_")
        || key.starts_with("__method_")
        || key.starts_with("__resume_")
}

const RT_ENQUEUE_PREFIX: &str = "rt_enqueue ";

pub fn rt_run_one_symbol(core: usize) -> String {
    format!("rt_run_one {core}")
}

pub fn rt_select_and_run_symbol(actor: &str) -> String {
    format!("rt_select_and_run {actor}")
}

pub fn rt_xreply_symbol(src_core: usize, dst_core: usize) -> String {
    format!("rt_xreply {src_core}->{dst_core}")
}

pub fn rt_xreply_cores(key: &str) -> Option<(usize, usize)> {
    let rest = key.strip_prefix("rt_xreply ")?;
    let (src, dst) = rest.split_once("->")?;
    Some((src.parse().ok()?, dst.parse().ok()?))
}

pub fn rt_child_poll_symbol(callee: &str) -> String {
    format!("rt_child_poll {callee}")
}

pub fn rt_drain_symbol(core: usize) -> String {
    format!("rt_drain {core}")
}

pub fn rt_xsend_symbol(src_core: usize, actor: &str) -> String {
    format!("rt_xsend {src_core} {actor}")
}

pub fn rt_secondary_core_entry_symbol(core: usize) -> String {
    format!("rt_secondary_core_entry {core}")
}

fn rt_run_one_glue_target(key: &str) -> bool {
    key.strip_prefix("rt_select_and_run ")
        .is_some_and(|a| !a.is_empty())
        || key
            .strip_prefix("rt_child_poll ")
            .is_some_and(|a| !a.is_empty())
        || key
            .strip_prefix("rt_drain ")
            .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

fn rt_select_and_run_glue_target(key: &str) -> bool {
    key.strip_prefix("rt_xreply ")
        .is_some_and(|rest| !rest.is_empty())
        || key
            .strip_prefix("__wrela_xreply_")
            .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

pub fn rt_boot_init_symbol() -> String {
    "rt_boot_init 0".to_string()
}

#[derive(Debug, Clone)]
pub struct BootInitSlotSpec {
    pub name: String,
    pub is_driver: bool,
    pub state_size: u64,
    pub init: Option<BootInitCallSpec>,
}

#[derive(Debug, Clone)]
pub struct BootInitCallSpec {
    pub key: String,
    pub args: Vec<BootInitArgSpec>,
    pub fallible: bool,
    pub err_msg: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootInitArgSpec {
    Word(u64),
    DeviceRegsBase(usize),
    PoolBase(String),
    OwnSlot {
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
    OwnHandleArray {
        pool: String,
        count: u64,
        slot_bytes: u64,
    },
}

pub const OFF_TURN_BUSY: u64 = 0;
pub const OFF_TURN_SUSPENDED: u64 = 8;
pub const OFF_TURN_RESUME_READY: u64 = 16;
pub const OFF_TURN_REPLY: u64 = 24;
pub const OFF_TURN_WAKER: u64 = 32;
pub const OFF_TURN_CUR_METHOD: u64 = 40;
pub const OFF_TURN_REPLY_SLOT: u64 = 48;
pub const OFF_TURN_REPLY_TAG: u64 = 56;
pub const TURN_RECORD_SIZE: u64 = 64;

pub const TURN_STATUS_COMPLETED: u64 = 0;
pub const TURN_STATUS_SUSPENDED: u64 = 1;
pub const TURN_STATUS_CANCELLED: u64 = 2;

pub const OFF_GROUP_IN_USE: u64 = 0;
pub const OFF_GROUP_CAPACITY: u64 = 8;
pub const OFF_GROUP_ACTIVE_CHILDREN: u64 = 16;
pub const OFF_GROUP_DEADLINE: u64 = 24;
pub const OFF_GROUP_CANCELLED: u64 = 32;
pub const OFF_GROUP_PARENT: u64 = 40;
pub const OFF_GROUP_JOIN_WAITER: u64 = 48;
pub const OFF_GROUP_OWNER_TURN: u64 = 56;
pub const OFF_GROUP_CHILDREN_BASE: u64 = 64;
pub const GROUP_MAX_CHILDREN_FLOOR: usize = 2;
pub const GROUP_SLOT_SIZE: u64 = OFF_GROUP_CHILDREN_BASE + (GROUP_MAX_CHILDREN_FLOOR as u64) * 16;

pub fn group_slot_size(max_children: usize) -> u64 {
    OFF_GROUP_CHILDREN_BASE + (max_children as u64) * 16
}
pub const GROUP_NO_PARENT: u64 = u64::MAX;

pub const CALL_ERROR_TAG_CANCELLED: u64 = 1;
pub const CALL_ERROR_TAG_NOT_ADMITTED: u64 = 3;
pub const ADMISSION_FULL: u64 = 0;
pub const REPLY_TAG_OK: u64 = 0;

pub fn group_child_tag_off(child_index: usize) -> u64 {
    OFF_GROUP_CHILDREN_BASE + (child_index as u64) * 16
}
pub fn group_child_payload_off(child_index: usize) -> u64 {
    group_child_tag_off(child_index) + 8
}

fn actor_of_method_key(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}
fn method_name_of_key(key: &str) -> &str {
    key.split('.').nth(1).unwrap_or(key)
}

pub type ActorMethodIndex = BTreeMap<String, BTreeMap<String, usize>>;

pub struct GroupCtx {
    pub arena_capacity: u64,
    pub max_children: usize,
    pub child_index: BTreeMap<String, usize>,
}

impl GroupCtx {
    pub fn slot_size(&self) -> u64 {
        group_slot_size(self.max_children)
    }
}

pub fn compute_group_child_indices(
    flow: &FlowWirProgram,
) -> Result<(BTreeMap<String, usize>, usize), CodegenError> {
    let mut out = BTreeMap::new();
    let mut max_children = GROUP_MAX_CHILDREN_FLOOR;
    for (_fn_key, f) in &flow.fns {
        let mut counters: BTreeMap<Temp, usize> = BTreeMap::new();
        for state in &f.states {
            for op in &state.ops {
                if let FlowInst::GroupStart {
                    group_temp,
                    callee_key,
                    ..
                } = op
                {
                    let counter = counters.entry(*group_temp).or_insert(0);
                    let this_idx = *counter;
                    *counter += 1;
                    if out.insert(callee_key.clone(), this_idx).is_some() {
                        return Err(CodegenError::unimplemented(&format!(
                            "async fn `{callee_key}` is `g.start`ed from more than one static \
                             call site (plans/M6.md item F's own disclosed floor: one free-turn \
                             area per fn, M6-C's own sizing)"
                        )));
                    }
                }
            }
        }
        for &count in counters.values() {
            if count > max_children {
                max_children = count;
            }
        }
    }
    Ok((out, max_children))
}

pub fn group_max_children_of(child_index: &BTreeMap<String, usize>) -> usize {
    child_index
        .values()
        .copied()
        .max()
        .map(|i| i + 1)
        .unwrap_or(0)
        .max(GROUP_MAX_CHILDREN_FLOOR)
}

enum FlatEntry {
    Op(FlowInst),
    Trans(Transition),
    AwaitResume {
        resume_state: usize,
        result_temp: Temp,
        what: AwaitKind,
    },
}

fn remap_local_jumps(op: &FlowInst, state_base: usize) -> FlowInst {
    match op {
        FlowInst::Mwir(Inst::Jump { target }) => FlowInst::Mwir(Inst::Jump {
            target: state_base + target,
        }),
        FlowInst::Mwir(Inst::JumpIfFalse { cond, target }) => FlowInst::Mwir(Inst::JumpIfFalse {
            cond: *cond,
            target: state_base + target,
        }),
        other => other.clone(),
    }
}

fn flatten(f: &FlowWirFn) -> (Vec<usize>, Vec<usize>, Vec<FlatEntry>) {
    let mut state_flat_base = Vec::with_capacity(f.states.len());
    let mut cursor = 0usize;
    for s in &f.states {
        state_flat_base.push(cursor);
        cursor += s.ops.len() + 1;
        if matches!(s.transition, Transition::Await { .. }) {
            cursor += 1;
        }
    }
    let mut resume_target = state_flat_base.clone();
    let mut flat = Vec::with_capacity(cursor);
    for (i, s) in f.states.iter().enumerate() {
        for op in &s.ops {
            flat.push(FlatEntry::Op(remap_local_jumps(op, state_flat_base[i])));
        }
        flat.push(FlatEntry::Trans(s.transition.clone()));
        if let Transition::Await {
            what,
            resume_state,
            result_temp,
        } = &s.transition
        {
            resume_target[*resume_state] = flat.len();
            flat.push(FlatEntry::AwaitResume {
                resume_state: *resume_state,
                result_temp: *result_temp,
                what: what.clone(),
            });
        }
    }
    (state_flat_base, resume_target, flat)
}

fn build_frame_flow(f: &FlowWirFn, layout: &LayoutCtx) -> Result<(Frame, Temp), CodegenError> {
    let mut temp_types = f.frame.temp_types.clone();
    let state_temp = Temp(temp_types.len());
    temp_types.push(Type::U64);
    let synthetic = MwirFn {
        receiver: f.receiver,
        params: f.params.clone(),
        ret: f.ret.clone(),
        temp_types,
        body: Vec::new(),
    };
    let frame = build_frame(
        &synthetic,
        layout,
        flow_reply_stage_size(f, layout)?,
        flow_entropy_scratch_size(f),
        TURN_RECORD_SIZE as usize,
        &regalloc::Assignment::none(synthetic.temp_types.len()),
        true,
    )?;
    Ok((frame, state_temp))
}

fn flow_entropy_scratch_size(f: &FlowWirFn) -> usize {
    f.states
        .iter()
        .flat_map(|s| s.ops.iter())
        .filter_map(|op| match op {
            FlowInst::Entropy { n, .. } => Some(*n as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn mwir_entropy_scratch_size(f: &MwirFn) -> usize {
    f.body
        .iter()
        .filter_map(|op| match op {
            Inst::Entropy { n, .. } => Some(*n as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn flow_reply_stage_size(f: &FlowWirFn, layout: &LayoutCtx) -> Result<usize, CodegenError> {
    let mut widest = 0usize;
    for s in &f.states {
        let Transition::Await {
            what, result_temp, ..
        } = &s.transition
        else {
            continue;
        };
        match what {
            AwaitKind::ActorCall { .. } => {
                let Some(declared) =
                    crate::sema::bodies::decompose_call_error(&f.frame.temp_types[result_temp.0])
                else {
                    continue;
                };
                if !is_aggregate(&declared) {
                    continue;
                }
                let sz = mwir::size_of(&declared, layout)
                    .map_err(|e| CodegenError::unimplemented(&e))?;
                widest = widest.max(sz);
            }
            AwaitKind::Receipt { .. } => {
                let ty = &f.frame.temp_types[result_temp.0];
                let sz = mwir::size_of(ty, layout).map_err(|e| CodegenError::unimplemented(&e))?;
                widest = widest.max(sz);
            }
            AwaitKind::GroupJoin { .. } => {}
        }
    }
    Ok(widest)
}

const BRK_ASYNC_DISPATCH_NO_STATE_MATCHED: u16 = 0xACD4;

fn emit_async_entry(
    f: &MwirFn,
    fn_key: &str,
    ctx: &mut FnCtx,
    state_temp: Temp,
    resume_target: &[usize],
) -> Result<(), CodegenError> {
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_FRAME, 0);
    for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
        w.text = format!("turn-frame[{i}] {} <{fn_key}>", reg_name(X_FRAME));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: fn_key.to_string(),
    });
    ctx.store_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
        format!(
            "ldr {}, [{}, #{OFF_TURN_SUSPENDED}]",
            reg_name(X_A),
            reg_name(X_FRAME)
        ),
        CostRule::Load,
        Some(X_A),
        &[X_FRAME],
    );
    let fork = ctx.emit_skip(SkipKind::Cbnz(X_A));

    let mut next_reg = 0u8;
    if let Some((self_temp, mode)) = f.receiver {
        let self_ty = &f.temp_types[self_temp.0];
        if is_aggregate(self_ty) || mode == AccessMode::Mut {
            let self_ptr_off = ctx
                .frame
                .self_ptr_off
                .ok_or_else(|| CodegenError::internal("receiver present but no self_ptr slot"))?;
            ctx.store_slot(next_reg, self_ptr_off);
            copy_self_fields_skipping_interrupt_cells(
                f,
                &ctx.frame,
                self_temp,
                ctx,
                SelfFieldCopy::LiveToFrame,
            )?;
        } else {
            ctx.store_slot(next_reg, ctx.frame.off(self_temp));
        }
        next_reg += 1;
    }
    let mut mut_ptr_iter = ctx.frame.mut_param_ptr_offs.iter();
    for (p, mode) in &f.params {
        if next_reg > 8 {
            return Err(CodegenError::unimplemented("more than 8 call arguments"));
        }
        let ty = &f.temp_types[p.0];
        if is_aggregate(ty) || *mode == AccessMode::Mut {
            if *mode == AccessMode::Mut {
                let (pt, ptr_off) = mut_ptr_iter.next().ok_or_else(|| {
                    CodegenError::internal("mut param missing from frame.mut_param_ptr_offs")
                })?;
                if *pt != *p {
                    return Err(CodegenError::internal(
                        "mut_param_ptr_offs order disagrees with MwirFn::params",
                    ));
                }
                ctx.store_slot(next_reg, *ptr_off);
            }
            let size = ctx.frame.size_of_temp(*p);
            let dst_off = ctx.frame.off(*p);
            let mut w = 0;
            while w < size {
                ctx.load_ptr(X_A, next_reg, w);
                ctx.store_slot(X_A, dst_off + w);
                w += 8;
            }
        } else {
            ctx.store_slot(next_reg, ctx.frame.off(*p));
        }
        next_reg += 1;
    }
    if mut_ptr_iter.next().is_some() {
        return Err(CodegenError::internal(
            "frame.mut_param_ptr_offs has more entries than Mut params",
        ));
    }
    if let Some(ret_ptr_off) = ctx.frame.ret_ptr_off {
        ctx.store_slot(8, ret_ptr_off);
    }
    ctx.load_imm(X_A, 0);
    ctx.store_slot(X_A, ctx.frame.off(state_temp));
    ctx.b_unconditional(0);

    ctx.patch_skip(fork, SkipKind::Cbnz(X_A));
    for off in [OFF_TURN_SUSPENDED, OFF_TURN_RESUME_READY] {
        ctx.push(
            encode::enc_str_x_imm(X_ZR, X_FRAME, off as u16),
            format!("str xzr, [{}, #{off}]", reg_name(X_FRAME)),
            CostRule::Store,
            None,
            &[X_ZR, X_FRAME],
        );
    }
    ctx.load_slot(X_A, ctx.frame.off(state_temp));
    for (i, &flat_idx) in resume_target.iter().enumerate() {
        ctx.push_flags(
            encode::enc_cmp_imm(X_A, i as u16, true),
            format!("cmp {}, #{i}", reg_name(X_A)),
            CostRule::Alu,
            None,
            &[X_A],
            FlagEffect::Write,
        );
        ctx.b_cond_to(Cond::Eq, flat_idx);
    }
    ctx.push(
        encode::enc_brk(BRK_ASYNC_DISPATCH_NO_STATE_MATCHED),
        format!("brk #{BRK_ASYNC_DISPATCH_NO_STATE_MATCHED:#x}"),
        CostRule::System,
        None,
        &[],
    );
    Ok(())
}

fn emit_async_epilogue(f: &MwirFn, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if is_aggregate(&f.ret) {
        ctx.push(
            encode::enc_mov_reg(1, X_ZR, true),
            "mov x1, xzr".to_string(),
            CostRule::Alu,
            Some(1),
            &[X_ZR],
        );
    } else {
        ctx.push(
            encode::enc_mov_reg(1, 0, true),
            "mov x1, x0".to_string(),
            CostRule::Alu,
            Some(1),
            &[0],
        );
    }
    ctx.push(
        encode::enc_str_x_imm(1, X_FRAME, OFF_TURN_REPLY as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_REPLY}]  ; complete → turn.reply",
            reg_name(1),
            reg_name(X_FRAME)
        ),
        CostRule::Store,
        None,
        &[1, X_FRAME],
    );
    if let Some((self_temp, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            copy_self_fields_skipping_interrupt_cells(
                f,
                ctx.frame,
                self_temp,
                ctx,
                SelfFieldCopy::FrameToLive,
            )?;
        }
    }
    for (p, ptr_off) in &ctx.frame.mut_param_ptr_offs {
        ctx.load_slot(X_A, *ptr_off);
        let size = ctx.frame.size_of_temp(*p);
        let src_off = ctx.frame.off(*p);
        let mut w = 0;
        while w < size {
            ctx.load_slot(X_B, src_off + w);
            ctx.store_ptr(X_B, X_A, w);
            w += 8;
        }
    }
    ctx.load_imm(0, TURN_STATUS_COMPLETED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
    Ok(())
}

impl FnCtx<'_> {
    fn b_cond_to(&mut self, cond: Cond, target_flat_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_flat_idx, this_word);
        let flags = match cond {
            Cond::Al | Cond::Nv => FlagEffect::None,
            _ => FlagEffect::Read,
        };
        self.push_flags(
            encode::enc_b_cond(cond, delta),
            format!("b.{} #{delta}", cond_mnemonic(cond)),
            CostRule::Branch,
            None,
            &[],
            flags,
        );
    }
}

fn emit_marshal_and_call(
    method_idx: usize,
    arg_temps: &[Temp],
    ctx: &mut FnCtx,
    symbol: &str,
    waker_self_key: Option<&str>,
) -> Result<(), CodegenError> {
    if arg_temps.len() > 2 {
        return Err(CodegenError::unimplemented(
            "more than 2 scalar message args (the by-value mailbox admission ABI carries x1/x2 only)",
        ));
    }
    for reg in [1u8, 2u8] {
        match arg_temps.get(reg as usize - 1) {
            Some(t) => ctx.load_slot(reg, ctx.frame.off(*t)),
            None => ctx.push(
                encode::enc_mov_reg(reg, X_ZR, true),
                format!("mov x{reg}, xzr"),
                CostRule::Alu,
                Some(reg),
                &[X_ZR],
            ),
        }
    }
    match waker_self_key {
        Some(fn_key) => {
            let word = ctx.cur_word();
            ctx.load_imm_naive(3, 0);
            for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                w.text = format!("turn-id[{i}] x3 <{fn_key}>");
            }
            ctx.relocs.push(Reloc::TurnIdImm {
                word,
                key: fn_key.to_string(),
            });
        }
        None => ctx.load_imm(3, 0),
    }
    ctx.load_imm(4, 0);
    ctx.load_imm(0, method_idx as i64);
    ctx.bl_symbolic_call(symbol, &[0, 1, 2, 3]);
    Ok(())
}

fn lookup_method_idx(
    method_key: &str,
    method_index: &ActorMethodIndex,
) -> Result<(String, usize), CodegenError> {
    let actor = actor_of_method_key(method_key).to_string();
    let method = method_name_of_key(method_key);
    let idx = method_index
        .get(&actor)
        .and_then(|m| m.get(method))
        .copied()
        .ok_or_else(|| {
            CodegenError::internal(format!(
                "unknown actor method `{method_key}` (no dispatch index)"
            ))
        })?;
    Ok((actor, idx))
}

fn emit_send(
    dst: Temp,
    method_key: &str,
    arg_temps: &[Temp],
    take_arg_temps: &[Temp],
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
) -> Result<(), CodegenError> {
    let (actor, idx) = lookup_method_idx(method_key, method_index)?;
    emit_marshal_and_call(idx, arg_temps, ctx, &rt_enqueue_symbol(&actor), None)?;
    let dst_off = ctx.frame.off(dst);
    let dst_size = ctx.frame.size_of_temp(dst);
    let skip_ok = ctx.emit_skip(SkipKind::Cbnz(0));
    let mut w = 0usize;
    while w < dst_size {
        ctx.store_slot(X_ZR, dst_off + w);
        w += 8;
    }
    let done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
    ctx.patch_skip(skip_ok, SkipKind::Cbnz(0));
    emit_not_admitted_local(ctx, dst_off, dst_size, take_arg_temps)?;
    ctx.patch_skip(done, SkipKind::Cond(Cond::Al));
    Ok(())
}

fn emit_self_path(
    dst: Temp,
    path: &[String],
    f: &MwirFn,
    ctx: &mut FnCtx,
) -> Result<(), CodegenError> {
    let (self_temp, _) = f
        .receiver
        .ok_or_else(|| CodegenError::internal("SelfPath op in a fn with no receiver"))?;
    let mut cur_off = ctx.frame.off(self_temp);
    let mut cur_ty = f.temp_types[self_temp.0].clone();
    for name in path {
        let base_ty = strip_wrappers(&cur_ty).clone();
        let Type::Named(sname, targs) = &base_ty else {
            return Err(CodegenError::internal(
                "SelfPath: an intermediate step is not a struct type",
            ));
        };
        let layout_key = if targs.is_empty() {
            sname.clone()
        } else {
            crate::sema::types::render_type(&Type::Named(sname.clone(), targs.clone()))
        };
        let names = ctx
            .layout
            .struct_field_names
            .get(&layout_key)
            .ok_or_else(|| {
                CodegenError::internal(format!(
                    "unknown struct `{layout_key}` (no field-name table)"
                ))
            })?;
        let idx = names.iter().position(|n| n == name).ok_or_else(|| {
            CodegenError::internal(format!("unknown field `{name}` on struct `{layout_key}`"))
        })?;
        let (off, _size) = field_offset_size(&base_ty, idx, ctx.layout)?;
        let field_ty = ctx.layout.structs[&layout_key][idx].clone();
        cur_off += off;
        cur_ty = field_ty;
    }
    let size = ctx.frame.size_of_temp(dst);
    ctx.copy_slot_to_slot(ctx.frame.off(dst), cur_off, size);
    Ok(())
}

fn emit_now(dst: Temp, ctx: &mut FnCtx) {
    ctx.load_imm(X_A, wrela_machine::mmio::CLOCK_MMIO_ADDR as i64);
    ctx.load_ptr(X_B, X_A, 0);
    ctx.store_slot(X_B, ctx.frame.off(dst));
}

fn emit_entropy(dst: Temp, n: u64, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    let scratch_off = ctx.frame.entropy_scratch_off.ok_or_else(|| {
        CodegenError::internal("entropy scratch not reserved in frame (codegen bug)")
    })?;
    if n == 0 || n as usize > ctx.frame.entropy_scratch_size {
        return Err(CodegenError::internal(format!(
            "entropy n={n} outside reserved scratch size {}",
            ctx.frame.entropy_scratch_size
        )));
    }
    let max = wrela_machine::machine_info::ENTROPY_LEN_MAX;
    if n > max {
        return Err(CodegenError::internal(format!(
            "entropy n={n} exceeds ENTROPY_LEN_MAX={max}"
        )));
    }

    ctx.addr_of_slot(X_A, scratch_off);
    ctx.load_imm(
        X_B,
        (wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_ENTROPY_DEST)
            as i64,
    );
    ctx.store_ptr(X_A, X_B, 0);

    ctx.load_imm(X_A, n as i64);
    ctx.load_imm(
        X_B,
        (wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_ENTROPY_LEN)
            as i64,
    );
    ctx.store_ptr(X_A, X_B, 0);

    ctx.load_imm(X_A, wrela_machine::mmio::ENTROPY_MMIO_ADDR as i64);
    ctx.store_ptr(X_ZR, X_A, 0);

    let dst_off = ctx.frame.off(dst);
    ctx.addr_of_slot(X_C, scratch_off);
    for i in 0..n as usize {
        ctx.load_byte_imm(X_B, X_C, i as u16);
        ctx.store_slot(X_B, dst_off + i * 8);
    }
    Ok(())
}

const LINEAGE_GROUP_SLOT: Temp = Temp(0);
const LINEAGE_DEADLINE_SLOT: Temp = Temp(1);

#[allow(clippy::too_many_arguments)]
fn emit_group_create(
    group_temp: Temp,
    capacity: Option<Temp>,
    deadline: Option<Temp>,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    fn_key: &str,
) -> Result<(), CodegenError> {
    const X_ARENA: u8 = 15;
    const X_CAND: u8 = 16;
    const X_TAG: u8 = 17;

    let word = ctx.cur_word();
    ctx.load_imm_naive(X_ARENA, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("group-arena-base {}", reg_name(X_ARENA));
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });

    ctx.load_slot(X_A, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.load_slot(X_B, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
    match deadline {
        Some(t) => ctx.load_slot(X_C, ctx.frame.off(t)),
        None => ctx.load_imm(X_C, 0),
    }
    let own_capacity_off = capacity.map(|t| ctx.frame.off(t));

    ctx.load_imm(X_D, u64::MAX as i64);
    ctx.push_flags(
        encode::enc_cmp_imm(X_B, 0, true),
        format!("cmp {}, #0", reg_name(X_B)),
        CostRule::Alu,
        None,
        &[X_B],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_csel(X_E, X_D, X_B, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_E),
            reg_name(X_D),
            reg_name(X_B)
        ),
        CostRule::Alu,
        Some(X_E),
        &[X_D, X_B],
        FlagEffect::Read,
    );
    ctx.push_flags(
        encode::enc_cmp_imm(X_C, 0, true),
        format!("cmp {}, #0", reg_name(X_C)),
        CostRule::Alu,
        None,
        &[X_C],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_csel(X_F, X_D, X_C, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_F),
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Alu,
        Some(X_F),
        &[X_D, X_C],
        FlagEffect::Read,
    );
    ctx.cmp_reg(X_E, X_F);
    ctx.push_flags(
        encode::enc_csel(X_TAG, X_E, X_F, Cond::Ls, true),
        format!(
            "csel {}, {}, {}, ls",
            reg_name(X_TAG),
            reg_name(X_E),
            reg_name(X_F)
        ),
        CostRule::Alu,
        Some(X_TAG),
        &[X_E, X_F],
        FlagEffect::Read,
    );
    ctx.cmp_reg(X_TAG, X_D);
    ctx.push_flags(
        encode::enc_csel(X_TAG, X_ZR, X_TAG, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_TAG),
            reg_name(X_ZR),
            reg_name(X_TAG)
        ),
        CostRule::Alu,
        Some(X_TAG),
        &[X_ZR, X_TAG],
        FlagEffect::Read,
    );
    ctx.push(
        encode::enc_sub_imm(X_B, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_B), reg_name(X_A)),
        CostRule::Alu,
        Some(X_B),
        &[X_A],
    );
    ctx.load_imm(X_D, GROUP_NO_PARENT as i64);
    ctx.push_flags(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
        CostRule::Alu,
        None,
        &[X_A],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_csel(X_B, X_D, X_B, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_B),
            reg_name(X_D),
            reg_name(X_B)
        ),
        CostRule::Alu,
        Some(X_B),
        &[X_D, X_B],
        FlagEffect::Read,
    );

    let mut to_after: Vec<usize> = Vec::new();
    for i in 0..gctx.arena_capacity {
        if i == 0 {
            ctx.push(
                encode::enc_add_imm(X_CAND, X_ARENA, 0, true),
                format!("add {}, {}, #0", reg_name(X_CAND), reg_name(X_ARENA)),
                CostRule::Alu,
                Some(X_CAND),
                &[X_ARENA],
            );
        } else {
            ctx.load_imm(X_D, (i * gctx.slot_size()) as i64);
            ctx.add_reg(X_CAND, X_ARENA, X_D);
        }
        ctx.push(
            encode::enc_ldr_x_imm(X_D, X_CAND, OFF_GROUP_IN_USE as u16),
            format!(
                "ldr {}, [{}, #{OFF_GROUP_IN_USE}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
            CostRule::Load,
            Some(X_D),
            &[X_CAND],
        );
        let skip_try_next = ctx.emit_skip(SkipKind::Cbnz(X_D));

        ctx.load_imm(X_D, 1);
        ctx.push(
            encode::enc_str_x_imm(X_D, X_CAND, OFF_GROUP_IN_USE as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_IN_USE}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_D, X_CAND],
        );
        match own_capacity_off {
            Some(off) => {
                ctx.load_slot(X_D, off);
            }
            None => ctx.load_imm(X_D, 0),
        }
        ctx.push(
            encode::enc_str_x_imm(X_D, X_CAND, OFF_GROUP_CAPACITY as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_CAPACITY}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_D, X_CAND],
        );
        for off in [OFF_GROUP_ACTIVE_CHILDREN, OFF_GROUP_CANCELLED] {
            ctx.push(
                encode::enc_str_x_imm(X_ZR, X_CAND, off as u16),
                format!("str xzr, [{}, #{off}]", reg_name(X_CAND)),
                CostRule::Store,
                None,
                &[X_ZR, X_CAND],
            );
        }
        ctx.push(
            encode::enc_str_w_imm(X_ZR, X_CAND, OFF_GROUP_JOIN_WAITER as u16),
            format!("str wzr, [{}, #{OFF_GROUP_JOIN_WAITER}]", reg_name(X_CAND)),
            CostRule::Store,
            None,
            &[X_ZR, X_CAND],
        );
        for c in 0..gctx.max_children {
            for off in [group_child_tag_off(c), group_child_payload_off(c)] {
                ctx.push(
                    encode::enc_str_x_imm(X_ZR, X_CAND, off as u16),
                    format!("str xzr, [{}, #{off}]", reg_name(X_CAND)),
                    CostRule::Store,
                    None,
                    &[X_ZR, X_CAND],
                );
            }
        }
        ctx.push(
            encode::enc_str_x_imm(X_TAG, X_CAND, OFF_GROUP_DEADLINE as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_DEADLINE}]",
                reg_name(X_TAG),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_TAG, X_CAND],
        );
        ctx.push(
            encode::enc_str_x_imm(X_B, X_CAND, OFF_GROUP_PARENT as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_PARENT}]",
                reg_name(X_B),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_B, X_CAND],
        );
        let word = ctx.cur_word();
        ctx.load_imm_naive(X_D, 0);
        for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
            w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_D));
        }
        ctx.relocs.push(Reloc::TurnIdImm {
            word,
            key: fn_key.to_string(),
        });
        ctx.push(
            encode::enc_str_w_imm(X_D, X_CAND, OFF_GROUP_OWNER_TURN as u16),
            format!(
                "str w{X_D}, [{}, #{OFF_GROUP_OWNER_TURN}]",
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_D, X_CAND],
        );
        ctx.load_imm(X_D, (i + 1) as i64);
        ctx.store_slot(X_D, ctx.frame.off(LINEAGE_GROUP_SLOT));
        ctx.store_slot(X_TAG, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
        ctx.store_slot(X_D, ctx.frame.off(group_temp));

        let j = ctx.words.len();
        ctx.words
            .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));
        to_after.push(j);
        ctx.patch_skip(skip_try_next, SkipKind::Cbnz(X_D));
    }
    if gctx.arena_capacity == 0 {
        ctx.abort_fixed("with group: arena capacity is zero (internal error)");
    } else {
        ctx.abort_fixed("with group: arena capacity exceeded (plans/M6.md item F)");
    }
    let after = ctx.cur_word();
    for j in to_after {
        let delta = (after as i64 - j as i64) as i32 * 4;
        ctx.words[j] = EmittedWord::new(
            encode::enc_b(delta),
            format!("b #{delta}"),
            CostRule::Branch,
            None,
            &[],
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_group_start(
    group_temp: Temp,
    callee_key: &str,
    arg_temps: &[Temp],
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    fn_key: &str,
) -> Result<(), CodegenError> {
    let child_index = *gctx.child_index.get(callee_key).ok_or_else(|| {
        CodegenError::internal(format!(
            "g.start callee `{callee_key}` has no child-slot ordinal (compute_group_child_indices \
             was not run over the whole program, or disagrees with this fn's own lowering)"
        ))
    })?;
    if arg_temps.len() > 2 {
        return Err(CodegenError::unimplemented(
            "more than 2 scalar `g.start` args (item C's own hand-assembled mailbox-slot floor)",
        ));
    }

    emit_group_addr_from_temp(ctx, group_temp, X_B, X_A, gctx);
    ctx.push(
        encode::enc_ldr_x_imm(X_C, X_B, OFF_GROUP_CANCELLED as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_CANCELLED}]",
            reg_name(X_C),
            reg_name(X_B)
        ),
        CostRule::Load,
        Some(X_C),
        &[X_B],
    );
    let skip_admit = ctx.emit_skip(SkipKind::Cbz(X_C));
    ctx.load_imm(X_A, 1);
    ctx.push(
        encode::enc_str_x_imm(X_A, X_B, group_child_tag_off(child_index) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_A),
            reg_name(X_B),
            group_child_tag_off(child_index)
        ),
        CostRule::Store,
        None,
        &[X_A, X_B],
    );
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_B, group_child_payload_off(child_index) as u16),
        format!(
            "str xzr, [{}, #{}]",
            reg_name(X_B),
            group_child_payload_off(child_index)
        ),
        CostRule::Store,
        None,
        &[X_ZR, X_B],
    );
    let to_after = ctx.words.len();
    ctx.words
        .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));
    ctx.patch_skip(skip_admit, SkipKind::Cbz(X_C));

    let word = ctx.cur_word();
    ctx.load_imm_naive(X_C, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-frame[{}] {} <{callee_key}>", 0, reg_name(X_C));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: callee_key.to_string(),
    });
    ctx.load_slot(X_D, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.push(
        encode::enc_str_x_imm(X_D, X_C, (TURN_RECORD_SIZE) as u16),
        format!(
            "str {}, [{}, #{TURN_RECORD_SIZE}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Store,
        None,
        &[X_D, X_C],
    );
    ctx.load_slot(X_D, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
    ctx.push(
        encode::enc_str_x_imm(X_D, X_C, (TURN_RECORD_SIZE + 8) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_D),
            reg_name(X_C),
            TURN_RECORD_SIZE + 8
        ),
        CostRule::Store,
        None,
        &[X_D, X_C],
    );
    ctx.load_imm(X_D, 1);
    ctx.push(
        encode::enc_str_x_imm(X_D, X_C, OFF_TURN_BUSY as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_BUSY}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Store,
        None,
        &[X_D, X_C],
    );
    for off in [OFF_TURN_SUSPENDED, OFF_TURN_RESUME_READY, OFF_TURN_WAKER] {
        ctx.push(
            encode::enc_str_x_imm(X_ZR, X_C, off as u16),
            format!("str xzr, [{}, #{off}]", reg_name(X_C)),
            CostRule::Store,
            None,
            &[X_ZR, X_C],
        );
    }

    let group_addr_reg = X_D;
    ctx.load_slot(X_E, ctx.frame.off(group_temp));
    ctx.push(
        encode::enc_sub_imm(X_E, X_E, 1, true),
        format!("sub {}, {}, #1", reg_name(X_E), reg_name(X_E)),
        CostRule::Alu,
        Some(X_E),
        &[X_E],
    );
    ctx.load_imm(X_F, gctx.slot_size() as i64);
    ctx.mul_reg(X_E, X_E, X_F);
    let word = ctx.cur_word();
    ctx.load_imm_naive(group_addr_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (g.start)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.add_reg(group_addr_reg, group_addr_reg, X_E);
    ctx.push(
        encode::enc_ldr_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Load,
        Some(X_A),
        &[group_addr_reg],
    );
    ctx.push(
        encode::enc_add_imm(X_A, X_A, 1, true),
        format!("add {}, {}, #1", reg_name(X_A), reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[X_A],
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "str {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Store,
        None,
        &[X_A, group_addr_reg],
    );

    for (i, t) in arg_temps.iter().enumerate() {
        ctx.load_slot(i as u8, ctx.frame.off(*t));
    }
    let arg_srcs: Vec<u8> = (0..arg_temps.len()).map(|i| i as u8).collect();
    ctx.bl_symbolic_call(callee_key, &arg_srcs);
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_FRAME, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-frame[{}] {} <{fn_key}>", 0, reg_name(X_FRAME));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: fn_key.to_string(),
    });
    ctx.load_slot(X_E, ctx.frame.off(group_temp));
    ctx.push(
        encode::enc_sub_imm(X_E, X_E, 1, true),
        format!("sub {}, {}, #1", reg_name(X_E), reg_name(X_E)),
        CostRule::Alu,
        Some(X_E),
        &[X_E],
    );
    ctx.load_imm(X_F, gctx.slot_size() as i64);
    ctx.mul_reg(X_E, X_E, X_F);
    let word = ctx.cur_word();
    ctx.load_imm_naive(group_addr_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (g.start harvest)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.add_reg(group_addr_reg, group_addr_reg, X_E);

    ctx.push_flags(
        encode::enc_cmp_imm(0, TURN_STATUS_SUSPENDED as u16, true),
        format!("cmp x0, #{TURN_STATUS_SUSPENDED}"),
        CostRule::Alu,
        None,
        &[0],
        FlagEffect::Write,
    );
    let skip_still_running = ctx.emit_skip(SkipKind::Cond(Cond::Eq));

    ctx.push_flags(
        encode::enc_cmp_imm(0, TURN_STATUS_CANCELLED as u16, true),
        format!("cmp x0, #{TURN_STATUS_CANCELLED}"),
        CostRule::Alu,
        None,
        &[0],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_cset(X_A, Cond::Eq, true),
        format!("cset {}, eq", reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[],
        FlagEffect::Read,
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, group_child_tag_off(child_index) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_A),
            reg_name(group_addr_reg),
            group_child_tag_off(child_index)
        ),
        CostRule::Store,
        None,
        &[X_A, group_addr_reg],
    );
    ctx.push(
        encode::enc_str_x_imm(
            1,
            group_addr_reg,
            group_child_payload_off(child_index) as u16,
        ),
        format!(
            "str x1, [{}, #{}]",
            reg_name(group_addr_reg),
            group_child_payload_off(child_index)
        ),
        CostRule::Store,
        None,
        &[1, group_addr_reg],
    );
    ctx.push(
        encode::enc_ldr_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Load,
        Some(X_A),
        &[group_addr_reg],
    );
    ctx.push(
        encode::enc_sub_imm(X_A, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_A), reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[X_A],
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "str {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Store,
        None,
        &[X_A, group_addr_reg],
    );
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_A, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-frame[{}] {} <{callee_key}>", 0, reg_name(X_A));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: callee_key.to_string(),
    });
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_A, OFF_TURN_BUSY as u16),
        format!("str xzr, [{}, #{OFF_TURN_BUSY}]", reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_ZR, X_A],
    );

    ctx.patch_skip(skip_still_running, SkipKind::Cond(Cond::Eq));
    let after = ctx.cur_word();
    let delta = (after as i64 - to_after as i64) as i32 * 4;
    ctx.words[to_after] = EmittedWord::new(
        encode::enc_b(delta),
        format!("b #{delta}"),
        CostRule::Branch,
        None,
        &[],
    );
    Ok(())
}

fn emit_group_close(
    group_temp: Temp,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
) -> Result<(), CodegenError> {
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_A, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (GroupClose)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.load_slot(X_B, ctx.frame.off(group_temp));
    ctx.push(
        encode::enc_sub_imm(X_B, X_B, 1, true),
        format!("sub {}, {}, #1", reg_name(X_B), reg_name(X_B)),
        CostRule::Alu,
        Some(X_B),
        &[X_B],
    );
    ctx.load_imm(X_C, gctx.slot_size() as i64);
    ctx.mul_reg(X_B, X_B, X_C);
    ctx.add_reg(X_A, X_A, X_B);
    ctx.push(
        encode::enc_ldr_x_imm(X_B, X_A, OFF_GROUP_PARENT as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_PARENT}]",
            reg_name(X_B),
            reg_name(X_A)
        ),
        CostRule::Load,
        Some(X_B),
        &[X_A],
    );
    ctx.load_imm(X_C, GROUP_NO_PARENT as i64);
    ctx.cmp_reg(X_B, X_C);
    let skip_no_parent = ctx.emit_skip(SkipKind::Cond(Cond::Eq));

    ctx.push(
        encode::enc_add_imm(X_B, X_B, 1, true),
        format!("add {}, {}, #1", reg_name(X_B), reg_name(X_B)),
        CostRule::Alu,
        Some(X_B),
        &[X_B],
    );
    ctx.store_slot(X_B, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.push(
        encode::enc_sub_imm(X_C, X_B, 1, true),
        format!("sub {}, {}, #1", reg_name(X_C), reg_name(X_B)),
        CostRule::Alu,
        Some(X_C),
        &[X_B],
    );
    ctx.load_imm(X_D, gctx.slot_size() as i64);
    ctx.mul_reg(X_C, X_C, X_D);
    let word2 = ctx.cur_word();
    ctx.load_imm_naive(X_D, 0);
    for w in ctx.words[word2..word2 + 4].iter_mut() {
        w.text = "group-arena-base (GroupClose parent deadline)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word: word2 });
    ctx.add_reg(X_C, X_D, X_C);
    ctx.push(
        encode::enc_ldr_x_imm(X_D, X_C, OFF_GROUP_DEADLINE as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_DEADLINE}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Load,
        Some(X_D),
        &[X_C],
    );
    ctx.store_slot(X_D, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
    let to_free = ctx.cur_word();
    ctx.words
        .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));

    ctx.patch_skip(skip_no_parent, SkipKind::Cond(Cond::Eq));
    ctx.store_slot(X_ZR, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.store_slot(X_ZR, ctx.frame.off(LINEAGE_DEADLINE_SLOT));

    let free = ctx.cur_word();
    let delta = (free as i64 - to_free as i64) as i32 * 4;
    ctx.words[to_free] = EmittedWord::new(
        encode::enc_b(delta),
        format!("b #{delta}"),
        CostRule::Branch,
        None,
        &[],
    );
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_A, OFF_GROUP_IN_USE as u16),
        format!("str xzr, [{}, #{OFF_GROUP_IN_USE}]", reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_ZR, X_A],
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_flow_op(
    op: &FlowInst,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
) -> Result<(), CodegenError> {
    match op {
        FlowInst::Mwir(inst) => emit_one(inst, f, ctx),
        FlowInst::SelfPath { dst, path } => emit_self_path(*dst, path, f, ctx),
        FlowInst::Now { dst } => {
            emit_now(*dst, ctx);
            Ok(())
        }
        FlowInst::Entropy { dst, n } => emit_entropy(*dst, *n, ctx),
        FlowInst::Duration { dst, n } => {
            const NS_PER_MS: i64 = 1_000_000;
            ctx.load_slot(X_A, ctx.frame.off(*n));
            ctx.load_imm(X_B, NS_PER_MS);
            ctx.mul_reg(X_A, X_A, X_B);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
            Ok(())
        }
        FlowInst::Send {
            dst,
            target: _,
            method_key,
            arg_temps,
            take_arg_temps,
        } => emit_send(
            *dst,
            method_key,
            arg_temps,
            take_arg_temps,
            ctx,
            method_index,
        ),
        FlowInst::GroupCreate {
            group_temp,
            capacity,
            deadline,
        } => emit_group_create(*group_temp, *capacity, *deadline, ctx, gctx, fn_key),
        FlowInst::GroupStart {
            group_temp,
            callee_key,
            arg_temps,
        } => emit_group_start(*group_temp, callee_key, arg_temps, ctx, gctx, fn_key),
        FlowInst::GroupClose { group_temp, .. } => emit_group_close(*group_temp, ctx, gctx),
    }
}

fn emit_group_cancelled_flags(ctx: &mut FnCtx, fn_key: &str, gctx: &GroupCtx) {
    ctx.push(
        encode::enc_movz(X_C, 0, 0, true),
        format!("movz {}, #0", reg_name(X_C)),
        CostRule::MovWide,
        Some(X_C),
        &[],
    );
    ctx.push(
        encode::enc_movz(X_D, 0, 0, true),
        format!("movz {}, #0", reg_name(X_D)),
        CostRule::MovWide,
        Some(X_D),
        &[],
    );
    ctx.load_slot(X_A, ctx.frame.off(LINEAGE_GROUP_SLOT));
    let skip_no_group = ctx.emit_skip(SkipKind::Cbz(X_A));
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_B, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (cancel flags)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.push(
        encode::enc_sub_imm(X_A, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_A), reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[X_A],
    );
    ctx.load_imm(X_E, gctx.slot_size() as i64);
    ctx.mul_reg(X_A, X_A, X_E);
    ctx.add_reg(X_B, X_B, X_A);
    ctx.push(
        encode::enc_ldr_x_imm(X_A, X_B, OFF_GROUP_CANCELLED as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_CANCELLED}]",
            reg_name(X_A),
            reg_name(X_B)
        ),
        CostRule::Load,
        Some(X_A),
        &[X_B],
    );
    ctx.push_flags(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
        CostRule::Alu,
        None,
        &[X_A],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_cset(X_C, Cond::Ne, true),
        format!("cset {}, ne", reg_name(X_C)),
        CostRule::Alu,
        Some(X_C),
        &[],
        FlagEffect::Read,
    );
    ctx.push(
        encode::enc_ldr_w_imm(X_A, X_B, OFF_GROUP_OWNER_TURN as u16),
        format!("ldr w{X_A}, [{}, #{OFF_GROUP_OWNER_TURN}]", reg_name(X_B)),
        CostRule::Load,
        Some(X_A),
        &[X_B],
    );
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_E, 0);
    for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
        w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_E));
    }
    ctx.relocs.push(Reloc::TurnIdImm {
        word,
        key: fn_key.to_string(),
    });
    ctx.push_flags(
        encode::enc_cmp_reg(X_A, X_E, false),
        format!("cmp w{X_A}, w{X_E}"),
        CostRule::Alu,
        None,
        &[X_A, X_E],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_cset(X_D, Cond::Eq, true),
        format!("cset {}, eq", reg_name(X_D)),
        CostRule::Alu,
        Some(X_D),
        &[],
        FlagEffect::Read,
    );
    ctx.patch_skip(skip_no_group, SkipKind::Cbz(X_A));
}

fn emit_checkpoint_cancellation_test(ctx: &mut FnCtx, gctx: &GroupCtx, fn_key: &str) {
    if gctx.arena_capacity == 0 {
        return;
    }
    let cancelled_tail = ctx.word_offsets.len() - 1;
    emit_group_cancelled_flags(ctx, fn_key, gctx);
    let skip_not_cancelled = ctx.emit_skip(SkipKind::Cbz(X_C));
    let skip_is_owner = ctx.emit_skip(SkipKind::Cbnz(X_D));
    ctx.b_unconditional(cancelled_tail);
    ctx.patch_skip(skip_is_owner, SkipKind::Cbnz(X_D));
    ctx.patch_skip(skip_not_cancelled, SkipKind::Cbz(X_C));
}

fn emit_async_cancelled_tail(ctx: &mut FnCtx) {
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_FRAME, OFF_TURN_REPLY as u16),
        format!(
            "str xzr, [{}, #{OFF_TURN_REPLY}]  ; cancelled → turn.reply = 0",
            reg_name(X_FRAME)
        ),
        CostRule::Store,
        None,
        &[X_ZR, X_FRAME],
    );
    ctx.load_imm(0, TURN_STATUS_CANCELLED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
}

fn emit_compose_group_join_result(
    ctx: &mut FnCtx,
    group_reg: u8,
    result_temp: Temp,
    child_count: usize,
) -> Result<(), CodegenError> {
    const VAL_TAG: u8 = X_C;
    const VAL_PAYLOAD: u8 = X_D;
    const VAL_CONST: u8 = X_E;
    if group_reg == VAL_TAG || group_reg == VAL_PAYLOAD || group_reg == VAL_CONST {
        return Err(CodegenError::internal(format!(
            "`g.join_all()` composition: the group-address register {} is one of the value \
             registers this loop loads into, so it would be clobbered mid-loop",
            reg_name(group_reg)
        )));
    }
    if child_count == 0 {
        return Ok(());
    }
    let total = ctx.frame.size_of_temp(result_temp);
    if total % child_count != 0 {
        return Err(CodegenError::internal(format!(
            "`g.join_all()`'s own result array ({total} bytes) does not divide evenly into \
             {child_count} elements"
        )));
    }
    let elem_size = total / child_count;
    const PAYLOAD_OFF: usize = 8;
    if elem_size < PAYLOAD_OFF + 8 {
        return Err(CodegenError::internal(format!(
            "`g.join_all()`'s own composed element is {elem_size} bytes — too small to hold a \
             tag plus one payload word"
        )));
    }
    let result_off = ctx.frame.off(result_temp);
    for c in 0..child_count {
        let elem_off = result_off + c * elem_size;
        ctx.push(
            encode::enc_ldr_x_imm(VAL_TAG, group_reg, group_child_tag_off(c) as u16),
            format!(
                "ldr {}, [{}, #{}]",
                reg_name(VAL_TAG),
                reg_name(group_reg),
                group_child_tag_off(c)
            ),
            CostRule::Load,
            Some(VAL_TAG),
            &[group_reg],
        );
        ctx.push(
            encode::enc_ldr_x_imm(VAL_PAYLOAD, group_reg, group_child_payload_off(c) as u16),
            format!(
                "ldr {}, [{}, #{}]",
                reg_name(VAL_PAYLOAD),
                reg_name(group_reg),
                group_child_payload_off(c)
            ),
            CostRule::Load,
            Some(VAL_PAYLOAD),
            &[group_reg],
        );
        ctx.load_imm(VAL_CONST, CALL_ERROR_TAG_CANCELLED as i64);
        ctx.push_flags(
            encode::enc_cmp_imm(VAL_TAG, 0, true),
            format!("cmp {}, #0", reg_name(VAL_TAG)),
            CostRule::Alu,
            None,
            &[VAL_TAG],
            FlagEffect::Write,
        );
        ctx.push_flags(
            encode::enc_csel(VAL_PAYLOAD, VAL_PAYLOAD, VAL_CONST, Cond::Eq, true),
            format!(
                "csel {}, {}, {}, eq",
                reg_name(VAL_PAYLOAD),
                reg_name(VAL_PAYLOAD),
                reg_name(VAL_CONST)
            ),
            CostRule::Alu,
            Some(VAL_PAYLOAD),
            &[VAL_PAYLOAD, VAL_CONST],
            FlagEffect::Read,
        );
        ctx.store_slot(VAL_TAG, elem_off);
        ctx.store_slot(VAL_PAYLOAD, elem_off + PAYLOAD_OFF);
        let mut w = PAYLOAD_OFF + 8;
        while w < elem_size {
            ctx.store_slot(X_ZR, elem_off + w);
            w += 8;
        }
    }
    Ok(())
}

fn emit_group_addr_from_temp(
    ctx: &mut FnCtx,
    group_temp: Temp,
    dst_reg: u8,
    scratch_reg: u8,
    gctx: &GroupCtx,
) {
    let word = ctx.cur_word();
    ctx.load_imm_naive(dst_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (join_all)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.load_slot(scratch_reg, ctx.frame.off(group_temp));
    ctx.push(
        encode::enc_sub_imm(scratch_reg, scratch_reg, 1, true),
        format!(
            "sub {}, {}, #1",
            reg_name(scratch_reg),
            reg_name(scratch_reg)
        ),
        CostRule::Alu,
        Some(scratch_reg),
        &[scratch_reg],
    );
    ctx.load_imm(X_D, gctx.slot_size() as i64);
    ctx.mul_reg(scratch_reg, scratch_reg, X_D);
    ctx.add_reg(dst_reg, dst_reg, scratch_reg);
}

fn is_handoff_receipt_reply(ty: &Type) -> bool {
    matches!(ty, Type::Named(n, _) if n == "Receipt")
}

fn aggregate_reply_of_await(f: &MwirFn, result_temp: Temp) -> Option<Type> {
    let declared = crate::sema::bodies::decompose_call_error(&f.temp_types[result_temp.0])?;
    is_aggregate(&declared).then_some(declared)
}

#[allow(clippy::too_many_arguments)]
fn emit_await_suspend(
    what: &AwaitKind,
    resume_state: usize,
    result_temp: Temp,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
    state_temp: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match what {
        AwaitKind::ActorCall {
            target_temp: _,
            method_key,
            arg_temps,
            take_arg_temps,
        } => {
            let (actor, idx) = lookup_method_idx(method_key, method_index)?;
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            if aggregate_reply_of_await(f, result_temp).is_some() {
                let stage_off = ctx.frame.reply_stage_off.ok_or_else(|| {
                    CodegenError::internal(
                        "an `await` with an aggregate declared reply but no reply staging slot \
                         (`build_frame_flow`/`flow_reply_stage_size` disagree with this site)",
                    )
                })?;
                let word = ctx.cur_word();
                ctx.load_imm_naive(X_A, 0);
                for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                    w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_A));
                }
                ctx.relocs.push(Reloc::TurnIdImm {
                    word,
                    key: fn_key.to_string(),
                });
                ctx.push(
                    encode::enc_str_w_imm(X_A, X_FRAME, OFF_TURN_REPLY_SLOT as u16),
                    format!(
                        "str w{X_A}, [{}, #{OFF_TURN_REPLY_SLOT}]",
                        reg_name(X_FRAME)
                    ),
                    CostRule::Store,
                    None,
                    &[X_A, X_FRAME],
                );
                let interior = (stage_off + ctx.slot_bias) as u16;
                ctx.push(
                    encode::enc_movz(X_A, interior, 0, false),
                    format!("movz w{X_A}, #{interior:#x}"),
                    CostRule::MovWide,
                    Some(X_A),
                    &[],
                );
                ctx.push(
                    encode::enc_str_w_imm(X_A, X_FRAME, OFF_TURN_REPLY_SLOT as u16 + 4),
                    format!(
                        "str w{X_A}, [{}, #{}]",
                        reg_name(X_FRAME),
                        OFF_TURN_REPLY_SLOT + 4
                    ),
                    CostRule::Store,
                    None,
                    &[X_A, X_FRAME],
                );
            }
            emit_marshal_and_call(
                idx,
                arg_temps,
                ctx,
                &rt_enqueue_symbol(&actor),
                Some(fn_key),
            )?;
            let composed_ty = &f.temp_types[result_temp.0];
            if is_handoff_receipt_reply(composed_ty) {
                let skip = ctx.emit_skip(SkipKind::Cbz(0));
                ctx.abort_fixed(&format!(
                    "await rejected: `{actor}`'s mailbox was full (a handoff `Receipt` has no \
                     CallError channel for NotAdmitted)"
                ));
                ctx.patch_skip(skip, SkipKind::Cbz(0));
            } else {
                let skip_admitted = ctx.emit_skip(SkipKind::Cbz(0));
                let result_off = ctx.frame.off(result_temp);
                let result_size = ctx.frame.size_of_temp(result_temp);
                emit_not_admitted_local(ctx, result_off, result_size, take_arg_temps)?;
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
                ctx.b_unconditional(state_flat_base[resume_state]);
                ctx.patch_skip(skip_admitted, SkipKind::Cbz(0));
            }
            emit_park_and_return(ctx);
            Ok(())
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => {
            if *child_count > gctx.max_children {
                return Err(CodegenError::unimplemented(&format!(
                    "`g.join_all()` over more than {} children (image GROUP_MAX_CHILDREN fact, \
                     plans/M12.md item F)",
                    gctx.max_children
                )));
            }
            emit_group_addr_from_temp(ctx, *group_temp, X_B, X_A, gctx);
            ctx.push(
                encode::enc_ldr_x_imm(X_C, X_B, OFF_GROUP_ACTIVE_CHILDREN as u16),
                format!(
                    "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
                    reg_name(X_C),
                    reg_name(X_B)
                ),
                CostRule::Load,
                Some(X_C),
                &[X_B],
            );
            let skip_park = ctx.emit_skip(SkipKind::Cbnz(X_C));
            emit_compose_group_join_result(ctx, X_B, result_temp, *child_count)?;
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            ctx.patch_skip(skip_park, SkipKind::Cbnz(X_C));
            let word = ctx.cur_word();
            ctx.load_imm_naive(X_A, 0);
            for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_A));
            }
            ctx.relocs.push(Reloc::TurnIdImm {
                word,
                key: fn_key.to_string(),
            });
            ctx.push(
                encode::enc_str_w_imm(X_A, X_B, OFF_GROUP_JOIN_WAITER as u16),
                format!("str w{X_A}, [{}, #{OFF_GROUP_JOIN_WAITER}]", reg_name(X_B)),
                CostRule::Store,
                None,
                &[X_A, X_B],
            );
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            emit_park_and_return(ctx);
            Ok(())
        }
        AwaitKind::Receipt { receipt_temp } => {
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            let stage_off = ctx.frame.reply_stage_off.ok_or_else(|| {
                CodegenError::internal(
                    "`await receipt` needs a reply staging slot for `IoCompletion` \
                     (`flow_reply_stage_size` disagrees with this site)",
                )
            })?;
            let result_size = mwir::size_of(&f.temp_types[result_temp.0], ctx.layout)
                .map_err(|e| CodegenError::unimplemented(&e))?;
            ctx.load_slot(X_D, ctx.frame.off(*receipt_temp));
            let word = ctx.cur_word();
            ctx.load_imm_naive(X_A, 0);
            for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_A));
            }
            ctx.relocs.push(Reloc::TurnIdImm {
                word,
                key: fn_key.to_string(),
            });
            let interior = (stage_off + ctx.slot_bias) as u16;
            ctx.push(
                encode::enc_movz(X_B, interior, 0, false),
                format!("movz w{X_B}, #{interior:#x}"),
                CostRule::MovWide,
                Some(X_B),
                &[],
            );
            ctx.push(
                encode::enc_str_w_imm(X_A, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as u16),
                format!(
                    "str w{X_A}, [{}, #{}]",
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_REPLY_STAGE
                ),
                CostRule::Store,
                None,
                &[X_A, X_D],
            );
            ctx.push(
                encode::enc_str_w_imm(X_B, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as u16 + 4),
                format!(
                    "str w{X_B}, [{}, #{}]",
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_REPLY_STAGE + 4
                ),
                CostRule::Store,
                None,
                &[X_B, X_D],
            );
            ctx.push(
                encode::enc_str_w_imm(X_A, X_D, crate::virtqueue::SLOT_META_WAITER as u16),
                format!(
                    "str w{X_A}, [{}, #{}]",
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_WAITER
                ),
                CostRule::Store,
                None,
                &[X_A, X_D],
            );
            ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
            ctx.load_imm(X_B, crate::virtqueue::SLOT_FLAG_RESOLVED as i64);
            ctx.and_reg(X_A, X_A, X_B);
            let need_park = ctx.emit_skip(SkipKind::Cbz(X_A));
            let stash_delta =
                crate::virtqueue::SLOT_META_BYTES + crate::virtqueue::REQ_HEADER_SIZE + 8;
            ctx.load_imm(X_A, stash_delta as i64);
            ctx.add_reg(X_A, X_D, X_A);
            let result_off = ctx.frame.off(result_temp);
            let mut w = 0usize;
            while w < result_size {
                ctx.load_ptr(X_B, X_A, w);
                ctx.store_slot(X_B, result_off + w);
                w += 8;
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            ctx.patch_skip(need_park, SkipKind::Cbz(X_A));
            emit_park_and_return(ctx);
            Ok(())
        }
    }
}

fn emit_park_and_return(ctx: &mut FnCtx) {
    ctx.load_imm(X_A, 1);
    ctx.push(
        encode::enc_str_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_SUSPENDED}]",
            reg_name(X_A),
            reg_name(X_FRAME)
        ),
        CostRule::Store,
        None,
        &[X_A, X_FRAME],
    );
    ctx.load_imm(0, TURN_STATUS_SUSPENDED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
}

fn emit_copy_staged_reply(
    ctx: &mut FnCtx,
    stage_off: usize,
    staged_size: usize,
    result_off: usize,
    result_size: usize,
) -> Result<(), CodegenError> {
    if staged_size + 8 > result_size {
        return Err(CodegenError::internal(format!(
            "a staged reply of {staged_size} byte(s) does not fit the composed result's own \
             {}-byte payload area (plans/M7.md item Z1: the staged declared reply must be \
             recomposed, not copied, when the two shapes differ)",
            result_size.saturating_sub(8)
        )));
    }
    let mut w = 0;
    while w < staged_size {
        ctx.load_slot(X_A, stage_off + w);
        ctx.store_slot(X_A, result_off + 8 + w);
        w += 8;
    }
    while w + 16 <= result_size {
        ctx.store_slot(X_ZR, result_off + 8 + w);
        w += 8;
    }
    Ok(())
}

fn emit_recompose_staged_result(
    ctx: &mut FnCtx,
    stage_off: usize,
    declared: &Type,
    composed_ty: &Type,
    result_off: usize,
    result_size: usize,
) -> Result<(), CodegenError> {
    let Type::Result(ok_ty, err_ty) = strip_wrappers(declared) else {
        return Err(CodegenError::internal(format!(
            "the staged declared reply is not a `Result`: {declared:?}"
        )));
    };
    let Type::Result(_, composed_err_ty) = strip_wrappers(composed_ty) else {
        return Err(CodegenError::internal(format!(
            "an actor await's composed result is not a `Result`: {composed_ty:?}"
        )));
    };
    let staged_payload_off = stage_off + enum_payload_offset(declared, 0, ctx.layout)?;
    let ok_payload_off = result_off + enum_payload_offset(composed_ty, 0, ctx.layout)?;
    let op_payload_off = ok_payload_off + enum_payload_offset(composed_err_ty, 0, ctx.layout)?;
    let ok_size = mwir::size_of(ok_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
    let err_size =
        mwir::size_of(err_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
    let result_end = result_off + result_size;
    if ok_payload_off + ok_size > result_end || op_payload_off + err_size > result_end {
        return Err(CodegenError::internal(format!(
            "a recomposed `Result` reply does not fit its composed temp: ok {ok_size} byte(s) at \
             +{}, `CallError.Op` {err_size} byte(s) at +{}, temp {result_size} byte(s) \
             (plans/M7.md item Z2)",
            ok_payload_off - result_off,
            op_payload_off - result_off
        )));
    }
    ctx.store_slot(X_ZR, ok_payload_off);
    let mut w = 0;
    while w < err_size {
        ctx.load_slot(X_A, staged_payload_off + w);
        ctx.store_slot(X_A, op_payload_off + w);
        w += 8;
    }
    while op_payload_off + w + 8 <= result_end {
        ctx.store_slot(X_ZR, op_payload_off + w);
        w += 8;
    }
    ctx.load_imm(X_A, 1);
    ctx.store_slot(X_A, result_off);
    ctx.load_slot(X_B, stage_off);
    let skip_ok = ctx.emit_skip(SkipKind::Cbnz(X_B));
    let mut w = 0;
    while w < ok_size {
        ctx.load_slot(X_A, staged_payload_off + w);
        ctx.store_slot(X_A, ok_payload_off + w);
        w += 8;
    }
    while ok_payload_off + w + 8 <= result_end {
        ctx.store_slot(X_ZR, ok_payload_off + w);
        w += 8;
    }
    ctx.store_slot(X_ZR, result_off);
    ctx.patch_skip(skip_ok, SkipKind::Cbnz(X_B));
    Ok(())
}

fn emit_compose_staged_reply(
    ctx: &mut FnCtx,
    stage_off: usize,
    declared: &Type,
    composed_ty: &Type,
    result_off: usize,
    result_size: usize,
) -> Result<(), CodegenError> {
    if matches!(strip_wrappers(declared), Type::Result(_, _)) {
        return emit_recompose_staged_result(
            ctx,
            stage_off,
            declared,
            composed_ty,
            result_off,
            result_size,
        );
    }
    let staged_size =
        mwir::size_of(declared, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
    emit_copy_staged_reply(ctx, stage_off, staged_size, result_off, result_size)?;
    ctx.store_slot(X_ZR, result_off);
    Ok(())
}

fn emit_not_admitted_local(
    ctx: &mut FnCtx,
    result_off: usize,
    result_size: usize,
    take_arg_temps: &[Temp],
) -> Result<(), CodegenError> {
    for t in take_arg_temps {
        let sz = ctx.frame.size_of_temp(*t);
        if sz != 8 {
            return Err(CodegenError::unimplemented(
                "NotAdmitted take-arg handback for a non-scalar argument (plans/M13.md item H; \
                 spill aggregates on the fail branch is not implemented)",
            ));
        }
    }
    let mut w = 0usize;
    while w < result_size {
        ctx.store_slot(X_ZR, result_off + w);
        w += 8;
    }
    ctx.load_imm(X_A, 1);
    ctx.store_slot(X_A, result_off);
    ctx.load_imm(X_B, CALL_ERROR_TAG_NOT_ADMITTED as i64);
    ctx.store_slot(X_B, result_off + 8);
    ctx.load_imm(X_A, ADMISSION_FULL as i64);
    ctx.store_slot(X_A, result_off + 16);
    let mut off = 24usize;
    for t in take_arg_temps {
        if off + 8 > result_size {
            return Err(CodegenError::internal(
                "NotAdmitted take-arg tuple does not fit the composed CallError temp \
                 (size_of/compose_call_error disagree with this site)",
            ));
        }
        ctx.load_slot(X_A, ctx.frame.off(*t));
        ctx.store_slot(X_A, result_off + off);
        off += 8;
    }
    Ok(())
}

fn emit_compose_from_reply_tag(ctx: &mut FnCtx, result_off: usize, result_size: usize) {
    let skip_err = ctx.emit_skip(SkipKind::Cbnz(X_B));
    ctx.store_slot(X_A, result_off + 8);
    let mut w = 16;
    while w < result_size {
        ctx.store_slot(X_ZR, result_off + w);
        w += 8;
    }
    ctx.store_slot(X_ZR, result_off);
    let skip_done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
    ctx.patch_skip(skip_err, SkipKind::Cbnz(X_B));
    ctx.store_slot(X_B, result_off + 8);
    if result_size >= 24 {
        ctx.store_slot(X_A, result_off + 16);
    }
    w = 24;
    while w < result_size {
        ctx.store_slot(X_ZR, result_off + w);
        w += 8;
    }
    ctx.load_imm(X_A, 1);
    ctx.store_slot(X_A, result_off);
    ctx.patch_skip(skip_done, SkipKind::Cond(Cond::Al));
}

#[allow(clippy::too_many_arguments)]
fn emit_await_resume(
    resume_state: usize,
    result_temp: Temp,
    what: &AwaitKind,
    f: &MwirFn,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    fn_key: &str,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match what {
        AwaitKind::ActorCall { .. } => {
            let composed_ty = &f.temp_types[result_temp.0];
            if is_handoff_receipt_reply(composed_ty) {
                if gctx.arena_capacity != 0 {
                    return Err(CodegenError::unimplemented(
                        "a handoff `await` (03-hardware.md §5) inside an image that declares a \
                         `with group` — a cancelled handoff receipt has no `CallError` channel \
                         to resolve into and must go to 03-hardware.md §9's recovery turn \
                         (plans/M8.md item F)",
                    ));
                }
                let result_off = ctx.frame.off(result_temp);
                ctx.push(
                    encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_REPLY as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                    CostRule::Load,
                    Some(X_A),
                    &[X_FRAME],
                );
                ctx.store_slot(X_A, result_off);
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
                ctx.b_unconditional(state_flat_base[resume_state]);
                return Ok(());
            }
            if !matches!(composed_ty, Type::Result(_, _)) {
                return Err(CodegenError::internal(format!(
                    "Await's own result_temp is not a composed Result type: {composed_ty:?}"
                )));
            }
            let result_off = ctx.frame.off(result_temp);
            let result_size = ctx.frame.size_of_temp(result_temp);
            let staged = match aggregate_reply_of_await(f, result_temp) {
                None => None,
                Some(declared) => {
                    let off = ctx.frame.reply_stage_off.ok_or_else(|| {
                        CodegenError::internal(
                            "an `await` resume with an aggregate declared reply but no reply \
                             staging slot (`flow_reply_stage_size` disagrees with this site)",
                        )
                    })?;
                    Some((off, declared))
                }
            };
            if let Some((stage_off, declared)) = staged {
                if gctx.arena_capacity == 0 {
                    emit_compose_staged_reply(
                        ctx,
                        stage_off,
                        &declared,
                        composed_ty,
                        result_off,
                        result_size,
                    )?;
                } else {
                    let Type::Result(_, composed_err_ty) = strip_wrappers(composed_ty) else {
                        return Err(CodegenError::internal(format!(
                            "an actor await's composed result is not a `Result`: {composed_ty:?}"
                        )));
                    };
                    let call_error_off =
                        result_off + enum_payload_offset(composed_ty, 0, ctx.layout)?;
                    let op_payload_off =
                        call_error_off + enum_payload_offset(composed_err_ty, 0, ctx.layout)?;
                    emit_group_cancelled_flags(ctx, fn_key, gctx);
                    ctx.load_imm(X_A, CALL_ERROR_TAG_CANCELLED as i64);
                    ctx.store_slot(X_A, call_error_off);
                    let mut w = op_payload_off;
                    while w < result_off + result_size {
                        ctx.store_slot(X_ZR, w);
                        w += 8;
                    }
                    ctx.load_imm(X_A, 1);
                    ctx.store_slot(X_A, result_off);
                    let skip_ok = ctx.emit_skip(SkipKind::Cbnz(X_C));
                    emit_compose_staged_reply(
                        ctx,
                        stage_off,
                        &declared,
                        composed_ty,
                        result_off,
                        result_size,
                    )?;
                    ctx.patch_skip(skip_ok, SkipKind::Cbnz(X_C));
                }
            } else {
                if gctx.arena_capacity != 0 {
                    emit_group_cancelled_flags(ctx, fn_key, gctx);
                }
                ctx.push(
                    encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_REPLY as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                    CostRule::Load,
                    Some(X_A),
                    &[X_FRAME],
                );
                ctx.push(
                    encode::enc_ldr_x_imm(X_B, X_FRAME, OFF_TURN_REPLY_TAG as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY_TAG}]",
                        reg_name(X_B),
                        reg_name(X_FRAME)
                    ),
                    CostRule::Load,
                    Some(X_B),
                    &[X_FRAME],
                );
                if gctx.arena_capacity != 0 {
                    let skip_force = ctx.emit_skip(SkipKind::Cbz(X_C));
                    ctx.push_flags(
                        encode::enc_cmp_imm(X_B, 0, true),
                        format!("cmp {}, #0", reg_name(X_B)),
                        CostRule::Alu,
                        None,
                        &[X_B],
                        FlagEffect::Write,
                    );
                    let skip_keep = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
                    ctx.load_imm(X_B, CALL_ERROR_TAG_CANCELLED as i64);
                    ctx.load_imm(X_A, 0);
                    ctx.patch_skip(skip_keep, SkipKind::Cond(Cond::Ne));
                    ctx.patch_skip(skip_force, SkipKind::Cbz(X_C));
                }
                emit_compose_from_reply_tag(ctx, result_off, result_size);
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => {
            emit_group_addr_from_temp(ctx, *group_temp, X_B, X_A, gctx);
            emit_compose_group_join_result(ctx, X_B, result_temp, *child_count)?;
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
        AwaitKind::Receipt { .. } => {
            let stage_off = ctx.frame.reply_stage_off.ok_or_else(|| {
                CodegenError::internal(
                    "`await receipt` resume needs the reply staging slot \
                     (`flow_reply_stage_size` disagrees with this site)",
                )
            })?;
            let result_off = ctx.frame.off(result_temp);
            let result_size = mwir::size_of(&f.temp_types[result_temp.0], ctx.layout)
                .map_err(|e| CodegenError::unimplemented(&e))?;
            let mut w = 0usize;
            while w < result_size {
                ctx.load_slot(X_A, stage_off + w);
                ctx.store_slot(X_A, result_off + w);
                w += 8;
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_transition(
    t: &Transition,
    flat_idx: usize,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
    state_temp: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match t {
        Transition::Return(value) => emit_one(&Inst::Return { value: *value }, f, ctx),
        Transition::Jump(target_state) => {
            let target_flat = state_flat_base[*target_state];
            if target_flat <= flat_idx {
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            }
            ctx.b_unconditional(target_flat);
            Ok(())
        }
        Transition::Branch {
            cond_temp,
            then_state,
            else_state,
        } => {
            ctx.load_slot(X_A, ctx.frame.off(*cond_temp));
            ctx.cbz(X_A, state_flat_base[*else_state]);
            ctx.b_unconditional(state_flat_base[*then_state]);
            Ok(())
        }
        Transition::Abort { msg } => {
            ctx.abort_fixed(msg);
            Ok(())
        }
        Transition::Await {
            what,
            resume_state,
            result_temp,
        } => emit_await_suspend(
            what,
            *resume_state,
            *result_temp,
            f,
            ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            state_flat_base,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn emit_flat_entry(
    entry: &FlatEntry,
    flat_idx: usize,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
    state_temp: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match entry {
        FlatEntry::Op(op) => emit_flow_op(op, f, ctx, method_index, gctx, fn_key),
        FlatEntry::Trans(t) => emit_transition(
            t,
            flat_idx,
            f,
            ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            state_flat_base,
        ),
        FlatEntry::AwaitResume {
            resume_state,
            result_temp,
            what,
        } => emit_await_resume(
            *resume_state,
            *result_temp,
            what,
            f,
            ctx,
            gctx,
            fn_key,
            state_flat_base,
        ),
    }
}

fn emit_flowwir_fn(
    fn_key: &str,
    f: &FlowWirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
) -> Result<CodegenFn, CodegenError> {
    if is_aggregate(&f.ret) && f.receiver.is_none() {
        return Err(CodegenError::unimplemented(
            "a free (non-method) async fn returning an aggregate — a `@test(runtime)` root's own \
             driver has no reply staging slot to hand it, and a `g.start` child's result slot in \
             the group arena is one word wide (plans/M7.md item Z1 widened the actor-*method* \
             case; this one is not implemented)",
        ));
    }
    let (frame, state_temp) = build_frame_flow(f, layout)?;
    let (state_flat_base, resume_target, flat) = flatten(f);
    let total = flat.len();
    let block_ids = if block_count_instruments(fn_key) {
        assign_flat_block_ids(&flat, &state_flat_base)?
    } else {
        vec![None; flat.len()]
    };

    let synthetic = MwirFn {
        receiver: f.receiver,
        params: f.params.clone(),
        ret: f.ret.clone(),
        temp_types: {
            let mut t = f.frame.temp_types.clone();
            t.push(Type::U64);
            t.push(Type::U64);
            t.push(Type::U64);
            t
        },
        body: vec![Inst::AssertFail { message: None }; total],
    };

    let dummy_targets = vec![0usize; total + 2];
    let mut probe_pro = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &dummy_targets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_FRAME,
        slot_bias: TURN_RECORD_SIZE as usize,
        cold_seq: 0,
        slot_accesses: Vec::new(),
        resident_misuse: None,
        home_mask: frame.home_mask(),
        home_def_ok: None,
        elide_branch: false,
    };
    emit_async_entry(
        &synthetic,
        fn_key,
        &mut probe_pro,
        state_temp,
        &resume_target,
    )?;
    let prologue_len = probe_pro.words.len();
    let elide = flat_branch_elision(&flat, &state_flat_base);
    let mut counts = Vec::with_capacity(total);
    for (i, entry) in flat.iter().enumerate() {
        let mut probe = FnCtx {
            frame: &frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_FRAME,
            slot_bias: TURN_RECORD_SIZE as usize,
            cold_seq: 0,
            slot_accesses: Vec::new(),
            resident_misuse: None,
            home_mask: frame.home_mask(),
            home_def_ok: None,
            elide_branch: elide[i],
        };
        if let Some(id) = block_ids[i] {
            if block_count() {
                probe.emit_block_hit(id);
            }
        }
        emit_flat_entry(
            entry,
            i,
            &synthetic,
            &mut probe,
            method_index,
            gctx,
            fn_key,
            state_temp,
            &state_flat_base,
        )?;
        counts.push(probe.words.len());
    }
    let mut word_offsets = vec![0usize; total + 2];
    let mut acc = prologue_len;
    for (i, c) in counts.iter().enumerate() {
        word_offsets[i] = acc;
        acc += c;
    }
    word_offsets[total] = acc;

    let mut probe_epi = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &dummy_targets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_FRAME,
        slot_bias: TURN_RECORD_SIZE as usize,
        cold_seq: 0,
        slot_accesses: Vec::new(),
        resident_misuse: None,
        home_mask: frame.home_mask(),
        home_def_ok: None,
        elide_branch: false,
    };
    emit_async_epilogue(&synthetic, &mut probe_epi)?;
    word_offsets[total + 1] = acc + probe_epi.words.len();

    let mut ctx = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &word_offsets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_FRAME,
        slot_bias: TURN_RECORD_SIZE as usize,
        cold_seq: 0,
        slot_accesses: Vec::new(),
        resident_misuse: None,
        home_mask: frame.home_mask(),
        home_def_ok: None,
        elide_branch: false,
    };
    emit_async_entry(&synthetic, fn_key, &mut ctx, state_temp, &resume_target)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for (i, entry) in flat.iter().enumerate() {
        if let Some(id) = block_ids[i] {
            if block_count() {
                ctx.emit_block_hit(id);
            }
        }
        ctx.elide_branch = elide[i];
        emit_flat_entry(
            entry,
            i,
            &synthetic,
            &mut ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            &state_flat_base,
        )?;
        ctx.elide_branch = false;
    }
    debug_assert_eq!(ctx.words.len(), word_offsets[total]);
    emit_async_epilogue(&synthetic, &mut ctx)?;
    debug_assert_eq!(ctx.words.len(), word_offsets[total + 1]);
    if gctx.arena_capacity > 0 {
        emit_async_cancelled_tail(&mut ctx);
    }

    if block_bridge() {
        record_spans(fn_key, &block_ids, &word_offsets, ctx.words.len());
    }

    Ok(CodegenFn {
        frame_size: frame.size,
        code: ctx.words,
        relocs: ctx.relocs,
    })
}

fn flat_branch_elision(flat: &[FlatEntry], state_flat_base: &[usize]) -> Vec<bool> {
    let n = flat.len();
    let leaders = flat_block_leaders(flat, state_flat_base);
    plan_branch_elision(n, &leaders, |i| match &flat[i] {
        FlatEntry::Op(FlowInst::Mwir(Inst::Jump { target })) => Some(*target),
        FlatEntry::Op(FlowInst::Mwir(Inst::Return { .. })) => Some(n),
        FlatEntry::Trans(Transition::Return(_)) => Some(n),
        _ => None,
    })
}

fn flat_block_leaders(flat: &[FlatEntry], state_flat_base: &[usize]) -> Vec<bool> {
    let n = flat.len();
    let mut leaders = vec![false; n];
    if n == 0 {
        return leaders;
    }
    leaders[0] = true;
    for &b in state_flat_base {
        if b < n {
            leaders[b] = true;
        }
    }
    for (i, entry) in flat.iter().enumerate() {
        match entry {
            FlatEntry::Op(FlowInst::Mwir(Inst::Jump { target })) => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Op(FlowInst::Mwir(Inst::JumpIfFalse { target, .. })) => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Op(FlowInst::Mwir(Inst::Return { .. })) => {
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Trans(Transition::Jump(state)) => {
                if let Some(&t) = state_flat_base.get(*state) {
                    if t < n {
                        leaders[t] = true;
                    }
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Trans(Transition::Branch {
                then_state,
                else_state,
                ..
            }) => {
                if let Some(&t) = state_flat_base.get(*then_state) {
                    if t < n {
                        leaders[t] = true;
                    }
                }
                if let Some(&e) = state_flat_base.get(*else_state) {
                    if e < n {
                        leaders[e] = true;
                    }
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Trans(
                Transition::Return(_) | Transition::Await { .. } | Transition::Abort { .. },
            ) => {
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::AwaitResume { resume_state, .. } => {
                leaders[i] = true;
                if let Some(&t) = state_flat_base.get(*resume_state) {
                    if t < n {
                        leaders[t] = true;
                    }
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            _ => {}
        }
    }
    leaders
}

fn assign_flat_block_ids(
    flat: &[FlatEntry],
    state_flat_base: &[usize],
) -> Result<Vec<Option<u32>>, CodegenError> {
    let mut ids = vec![None; flat.len()];
    if !block_ids_active() {
        return Ok(ids);
    }
    for (i, is_leader) in flat_block_leaders(flat, state_flat_base)
        .into_iter()
        .enumerate()
    {
        if is_leader {
            ids[i] = Some(alloc_block_id()?);
        }
    }
    Ok(ids)
}

pub fn async_frame_sizes(
    flow: &FlowWirProgram,
    layout: &LayoutCtx,
) -> Result<BTreeMap<String, u64>, CodegenError> {
    let mut out = BTreeMap::new();
    for (key, f) in &flow.fns {
        let (frame, _) = build_frame_flow(f, layout)?;
        out.insert(key.clone(), frame.size as u64);
    }
    Ok(out)
}

pub fn emit_secondary_sp_install(core: usize, n_cores: usize) -> Vec<EmittedWord> {
    let mut words: Vec<EmittedWord> = Vec::new();
    let push = |words: &mut Vec<EmittedWord>,
                w: u32,
                text: String,
                rule: CostRule,
                dst: Option<u8>,
                srcs: &[u8]| {
        words.push(EmittedWord::new(w, text, rule, dst, srcs));
    };
    let load_imm = |words: &mut Vec<EmittedWord>, reg: u8, value: u64, label: &str| {
        let h0 = (value & 0xFFFF) as u16;
        let h1 = ((value >> 16) & 0xFFFF) as u16;
        let h2 = ((value >> 32) & 0xFFFF) as u16;
        let h3 = ((value >> 48) & 0xFFFF) as u16;
        push(
            words,
            encode::enc_movz(reg, h0, 0, true),
            format!("movz {}, #{:#x}  ; {label}", reg_name(reg), value),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        push(
            words,
            encode::enc_movk(reg, h1, 16, true),
            format!("movk {}, #{:#x}, lsl #16", reg_name(reg), h1),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        push(
            words,
            encode::enc_movk(reg, h2, 32, true),
            format!("movk {}, #{:#x}, lsl #32", reg_name(reg), h2),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        push(
            words,
            encode::enc_movk(reg, h3, 48, true),
            format!("movk {}, #{:#x}, lsl #48", reg_name(reg), h3),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
    };
    let n = n_cores.max(1);
    let sp_top =
        wrela_machine::layout::core_stack_base_n(core, n) + wrela_machine::layout::CORE_STACK_SIZE;
    load_imm(&mut words, 9, sp_top, "sp_top");
    push(
        &mut words,
        encode::enc_add_imm(31, 9, 0, true),
        "mov sp, x9".to_string(),
        CostRule::Alu,
        Some(31),
        &[9],
    );
    words
}

fn push(
    words: &mut Vec<EmittedWord>,
    w: u32,
    text: String,
    rule: CostRule,
    dst: Option<u8>,
    srcs: &[u8],
) {
    words.push(EmittedWord::new(w, text, rule, dst, srcs));
}

fn push_rodata_addr(
    words: &mut Vec<EmittedWord>,
    relocs: &mut Vec<Reloc>,
    reg: u8,
    byte_offset: usize,
    off_text: &str,
) {
    let word = words.len();
    if adr_addressing() {
        push(
            words,
            encode::enc_adr(reg, 0),
            format!("adr {}, rodata+{off_text}", reg_name(reg)),
            CostRule::Adrp,
            Some(reg),
            &[],
        );
        relocs.push(Reloc::RodataAdr { word, byte_offset });
        return;
    }
    push(
        words,
        encode::enc_adrp(reg, 0),
        format!("adrp {}, rodata+{off_text}", reg_name(reg)),
        CostRule::Adrp,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_add_imm(reg, reg, 0, true),
        format!(
            "add {}, {}, #rodata+{off_text}",
            reg_name(reg),
            reg_name(reg)
        ),
        CostRule::Alu,
        Some(reg),
        &[reg],
    );
    relocs.push(Reloc::Rodata {
        word_adrp: word,
        byte_offset,
    });
}

fn load_imm(words: &mut Vec<EmittedWord>, reg: u8, value: u64, label: &str) {
    let h0 = (value & 0xFFFF) as u16;
    let h1 = ((value >> 16) & 0xFFFF) as u16;
    let h2 = ((value >> 32) & 0xFFFF) as u16;
    let h3 = ((value >> 48) & 0xFFFF) as u16;
    push(
        words,
        encode::enc_movz(reg, h0, 0, true),
        format!("movz {}, #{:#x}  ; {label}", reg_name(reg), value),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_movk(reg, h1, 16, true),
        format!("movk {}, #{:#x}, lsl #16", reg_name(reg), h1),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_movk(reg, h2, 32, true),
        format!("movk {}, #{:#x}, lsl #32", reg_name(reg), h2),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_movk(reg, h3, 48, true),
        format!("movk {}, #{:#x}, lsl #48", reg_name(reg), h3),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
}

pub fn emit_boot_init_call(slot: &BootInitSlotSpec) -> CodegenFn {
    fn load_state(
        words: &mut Vec<EmittedWord>,
        relocs: &mut Vec<Reloc>,
        reg: u8,
        slot: &BootInitSlotSpec,
    ) {
        let word = words.len();
        load_imm(words, reg, 0, &format!("state {}", slot.name));
        for i in 0..4 {
            if let Some(ew) = words.get_mut(word + i) {
                ew.text = format!("state-addr[{i}] {} x{reg}", slot.name);
            }
        }
        if slot.is_driver {
            relocs.push(Reloc::DriverState {
                word,
                driver: slot.name.clone(),
            });
        } else {
            relocs.push(Reloc::MailboxAddr {
                word,
                actor: slot.name.clone(),
                field: MailboxField::State,
            });
        }
    }
    fn bl_key(words: &mut Vec<EmittedWord>, relocs: &mut Vec<Reloc>, key: &str) {
        let word = words.len();
        push(
            words,
            encode::enc_bl(0),
            format!("bl <{key}>"),
            CostRule::Call,
            Some(0),
            &[],
        );
        relocs.push(Reloc::Call {
            word,
            key: key.to_string(),
        });
    }
    fn emit_arg(
        words: &mut Vec<EmittedWord>,
        relocs: &mut Vec<Reloc>,
        reg: u8,
        arg: &BootInitArgSpec,
    ) -> Result<u64, String> {
        match arg {
            BootInitArgSpec::Word(w) => {
                load_imm(words, reg, *w, "init arg");
                Ok(0)
            }
            BootInitArgSpec::DeviceRegsBase(i) => {
                let word = words.len();
                load_imm(words, reg, 0, &format!("device#{i} regs"));
                for j in 0..4 {
                    if let Some(ew) = words.get_mut(word + j) {
                        ew.text = format!("device-regs[{j}] device#{i} x{reg}");
                    }
                }
                relocs.push(Reloc::DeviceRegsBase { word, device: *i });
                Ok(0)
            }
            BootInitArgSpec::PoolBase(name) => {
                let word = words.len();
                load_imm(words, reg, 0, &format!("pool {name}"));
                for j in 0..4 {
                    if let Some(ew) = words.get_mut(word + j) {
                        ew.text = format!("pool-base[{j}] {name} x{reg}");
                    }
                }
                relocs.push(Reloc::PoolBase {
                    word,
                    pool: name.clone(),
                });
                Ok(0)
            }
            BootInitArgSpec::OwnSlot {
                pool,
                index,
                slot_bytes,
            } => {
                let word = words.len();
                load_imm(words, reg, 0, &format!("own {pool}[{index}]"));
                for j in 0..4 {
                    if let Some(ew) = words.get_mut(word + j) {
                        ew.text = format!("pool-slot[{j}] {pool}[{index}] x{reg}");
                    }
                }
                relocs.push(Reloc::PoolSlot {
                    word,
                    pool: pool.clone(),
                    index: *index,
                    slot_bytes: *slot_bytes,
                });
                Ok(0)
            }
            BootInitArgSpec::OwnHandleArray {
                pool,
                count,
                slot_bytes,
            } => {
                let raw = count
                    .checked_mul(8)
                    .ok_or_else(|| "own-handle array byte count overflow".to_string())?;
                let bytes = ((raw + 15) / 16) * 16;
                if bytes == 0 || bytes >= 4096 {
                    return Err(format!(
                        "own-handle array for pool `{pool}` wants {bytes} bytes \
                         (count={count}); boot's unsigned-immediate SUB reaches 4095"
                    ));
                }
                push(
                    words,
                    encode::enc_sub_imm(31, 31, bytes as u16, true),
                    format!("sub sp, sp, #{bytes}  ; own-handle table"),
                    CostRule::Alu,
                    Some(31),
                    &[31],
                );
                for i in 0..*count {
                    let word = words.len();
                    load_imm(words, 9, 0, &format!("own {pool}[{i}]"));
                    for j in 0..4 {
                        if let Some(ew) = words.get_mut(word + j) {
                            ew.text = format!("pool-slot[{j}] {pool}[{i}] x9");
                        }
                    }
                    relocs.push(Reloc::PoolSlot {
                        word,
                        pool: pool.clone(),
                        index: i,
                        slot_bytes: *slot_bytes,
                    });
                    push(
                        words,
                        encode::enc_str_x_imm(9, 31, (i * 8) as u16),
                        format!("str x9, [sp, #{}]", i * 8),
                        CostRule::Store,
                        None,
                        &[9, 31],
                    );
                }
                push(
                    words,
                    encode::enc_add_imm(reg, 31, 0, true),
                    format!("mov {}, sp", reg_name(reg)),
                    CostRule::Alu,
                    Some(reg),
                    &[31],
                );
                Ok(bytes)
            }
        }
    }

    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();

    let Some(call) = &slot.init else {
        panic!("emit_boot_init_call: slot `{}` has no init", slot.name);
    };

    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".to_string(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".to_string(),
        CostRule::Store,
        None,
        &[30, 31],
    );

    let mut array_stack: u64 = 0;
    for (i, arg) in call.args.iter().enumerate() {
        match emit_arg(&mut words, &mut relocs, i as u8 + 1, arg) {
            Ok(n) => array_stack += n,
            Err(msg) => panic!("emit_boot_init_call: {msg}"),
        }
    }
    load_state(&mut words, &mut relocs, 0, slot);
    if call.fallible {
        let (msg_off, msg_len) = call.err_msg.unwrap_or_else(|| {
            panic!(
                "emit_boot_init_call: fallible `{}` has no interned abort message",
                call.key
            )
        });
        push(
            &mut words,
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; reply slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        push(
            &mut words,
            encode::enc_add_imm(8, 31, 0, true),
            "mov x8, sp".to_string(),
            CostRule::Alu,
            Some(8),
            &[31],
        );
        bl_key(&mut words, &mut relocs, &call.key);
        push(
            &mut words,
            encode::enc_ldr_x_imm(9, 31, 0),
            "ldr x9, [sp]  ; Result tag".to_string(),
            CostRule::Load,
            Some(9),
            &[31],
        );
        push(
            &mut words,
            encode::enc_add_imm(31, 31, 16, true),
            "add sp, sp, #16  ; drop reply slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        let ok_fixup = words.len();
        push(
            &mut words,
            0,
            "cbz x9, .ok".to_string(),
            CostRule::Branch,
            None,
            &[],
        );
        push(
            &mut words,
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; abort Bytes slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        push_rodata_addr(&mut words, &mut relocs, 10, msg_off, &format!("{msg_off}"));
        push(
            &mut words,
            encode::enc_str_x_imm(10, 31, 0),
            "str x10, [sp]  ; Bytes.base".to_string(),
            CostRule::Store,
            None,
            &[10, 31],
        );
        load_imm(&mut words, 10, msg_len as u64, "abort msg len");
        push(
            &mut words,
            encode::enc_str_x_imm(10, 31, 8),
            "str x10, [sp, #8]  ; Bytes.len".to_string(),
            CostRule::Store,
            None,
            &[10, 31],
        );
        push(
            &mut words,
            encode::enc_add_imm(0, 31, 0, true),
            "add x0, sp, #0  ; *Bytes".to_string(),
            CostRule::Alu,
            Some(0),
            &[31],
        );
        let abort_word = words.len();
        push(
            &mut words,
            encode::enc_bl(0),
            "bl <__wrela_abort>".to_string(),
            CostRule::Abort,
            None,
            &[],
        );
        relocs.push(Reloc::AbortFixed { word: abort_word });
        let after = words.len();
        let delta = (after as i64 - ok_fixup as i64) * 4;
        if let Some(ew) = words.get_mut(ok_fixup) {
            ew.word = encode::enc_cbz(9, delta as i32, true);
            ew.text = format!("cbz x9, .ok ({delta})");
            ew.rule = CostRule::Branch;
            ew.dst = None;
            ew.srcs = [9, 0, 0, 0];
            ew.src_len = 1;
        }
    } else {
        bl_key(&mut words, &mut relocs, &call.key);
    }
    if array_stack > 0 {
        assert!(
            array_stack < 4096,
            "own-handle array stack frame is {array_stack} bytes"
        );
        push(
            &mut words,
            encode::enc_add_imm(31, 31, array_stack as u16, true),
            format!("add sp, sp, #{array_stack}  ; free own-handle table"),
            CostRule::Alu,
            Some(31),
            &[31],
        );
    }

    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".to_string(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".to_string(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[30],
    );

    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointIrqSpec {
    pub vector: u64,
    pub handler_key: String,
    pub driver_state: u64,
}

#[derive(Debug, Clone)]
pub struct CheckpointWakeSpec {
    pub driver_state: u64,
    pub wake_pending_off: u64,
    pub task_key: String,
}

pub struct CheckpointEmitResult {
    pub words: Vec<u32>,
    pub checkpoint_service_word: usize,
    pub deadline_poll_word: Option<usize>,
    pub has_deadline_poll: bool,
    pub relocs: Vec<Reloc>,
}

pub fn emit_checkpoint_lr_frame() -> Vec<EmittedWord> {
    vec![
        EmittedWord::new(
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; floor cat2".into(),
            CostRule::Alu,
            Some(31),
            &[31],
        ),
        EmittedWord::new(
            encode::enc_str_x_imm(30, 31, 0),
            "str x30, [sp]  ; floor cat2".into(),
            CostRule::Store,
            None,
            &[30, 31],
        ),
        EmittedWord::new(
            encode::enc_ldr_x_imm(30, 31, 0),
            "ldr x30, [sp]  ; floor cat2".into(),
            CostRule::Load,
            Some(30),
            &[31],
        ),
        EmittedWord::new(
            encode::enc_add_imm(31, 31, 16, true),
            "add sp, sp, #16  ; floor cat2".into(),
            CostRule::Alu,
            Some(31),
            &[31],
        ),
        EmittedWord::new(
            encode::enc_ret(30),
            "ret  ; floor cat2".into(),
            CostRule::Branch,
            None,
            &[30],
        ),
    ]
}

pub fn emit_checkpoint_service_trampoline(
    has_deadline_poll: bool,
    link_body: bool,
) -> CheckpointEmitResult {
    if !link_body {
        return CheckpointEmitResult {
            words: vec![encode::enc_ret(30)],
            checkpoint_service_word: 0,
            deadline_poll_word: None,
            has_deadline_poll,
            relocs: vec![],
        };
    }
    let frame = emit_checkpoint_lr_frame();
    debug_assert_eq!(frame.len(), 5);
    let mut words = Vec::new();
    let mut relocs = Vec::new();
    words.push(frame[0].word);
    words.push(frame[1].word);
    words.push(encode::enc_movz(0, 0, 0, true));
    let bl_word = words.len();
    words.push(encode::enc_bl(0));
    relocs.push(Reloc::Call {
        word: bl_word,
        key: "__wrela_rt_checkpoint".into(),
    });
    words.push(frame[2].word);
    words.push(frame[3].word);
    words.push(frame[4].word);
    CheckpointEmitResult {
        words,
        checkpoint_service_word: 0,
        deadline_poll_word: None,
        has_deadline_poll,
        relocs,
    }
}

pub fn emit_checkpoint_irq_call(spec: &CheckpointIrqSpec) -> CodegenFn {
    emit_driver_state_call(&spec.handler_key, spec.driver_state)
}

pub fn emit_checkpoint_wake_call(spec: &CheckpointWakeSpec) -> CodegenFn {
    emit_driver_state_call(&spec.task_key, spec.driver_state)
}

fn emit_driver_state_call(key: &str, driver_state: u64) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    load_imm(&mut words, 0, driver_state, "driver_state");
    let bl = words.len();
    push(
        &mut words,
        encode::enc_bl(0),
        format!("bl <{key}>"),
        CostRule::Call,
        Some(0),
        &[0],
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: key.to_string(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

pub fn emit_method_call_stub(method_key: &str, state: u64) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    push(
        &mut words,
        encode::enc_mov_reg(8, 2, true),
        "mov x8, x2  ; aggregate stage".into(),
        CostRule::Alu,
        Some(8),
        &[2],
    );
    push(
        &mut words,
        encode::enc_mov_reg(2, 1, true),
        "mov x2, x1  ; arg1".into(),
        CostRule::Alu,
        Some(2),
        &[1],
    );
    push(
        &mut words,
        encode::enc_mov_reg(1, 0, true),
        "mov x1, x0  ; arg0".into(),
        CostRule::Alu,
        Some(1),
        &[0],
    );
    load_imm(&mut words, 0, state, "actor state");
    let bl = words.len();
    push(
        &mut words,
        encode::enc_bl(0),
        format!("bl <{method_key}>"),
        CostRule::Call,
        Some(0),
        &[0, 1, 2],
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: method_key.to_string(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

pub fn emit_test_call_stub(test_key: &str, args: &[u64]) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    for (i, &v) in args.iter().enumerate() {
        assert!(i < 8, "emit_test_call_stub: too many args");
        load_imm(&mut words, i as u8, v, &format!("test arg {i}"));
    }
    let line_buf =
        wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_TEST_LINE_BUF;
    load_imm(&mut words, 8, line_buf, "OFF_TEST_LINE_BUF");
    let bl = words.len();
    let mut call_srcs: Vec<u8> = (0..args.len().min(4)).map(|i| i as u8).collect();
    if call_srcs.len() < 4 {
        call_srcs.push(8);
    }
    push(
        &mut words,
        encode::enc_bl(0),
        format!("bl <{test_key}>"),
        CostRule::Call,
        Some(0),
        &call_srcs,
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: test_key.to_string(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

pub fn emit_test_prefix_stub(rodata_off: usize, len: u64) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 32, true),
        "sub sp, sp, #32".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 16),
        "str x30, [sp, #16]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    push_rodata_addr(
        &mut words,
        &mut relocs,
        9,
        rodata_off,
        &format!("{rodata_off:#x}"),
    );
    push(
        &mut words,
        encode::enc_str_x_imm(9, 31, 0),
        "str x9, [sp]  ; Bytes.base".into(),
        CostRule::Store,
        None,
        &[9, 31],
    );
    load_imm(&mut words, 9, len, "Bytes.capacity");
    push(
        &mut words,
        encode::enc_str_x_imm(9, 31, 8),
        "str x9, [sp, #8]  ; Bytes.len".into(),
        CostRule::Store,
        None,
        &[9, 31],
    );
    push(
        &mut words,
        encode::enc_add_imm(0, 31, 0, true),
        "add x0, sp, #0  ; *Bytes".into(),
        CostRule::Alu,
        Some(0),
        &[31],
    );
    load_imm(&mut words, 1, len, "copy len");
    let bl = words.len();
    push(
        &mut words,
        encode::enc_bl(0),
        "bl <__wrela_console_append_bytes>".into(),
        CostRule::Call,
        Some(0),
        &[0, 1],
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: "__wrela_console_append_bytes".into(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 16),
        "ldr x30, [sp, #16]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 32, true),
        "add sp, sp, #32".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 32,
        code: words,
        relocs,
    }
}

pub fn codegen_program_with_async(
    mwir: &MwirProgram,
    flow: &FlowWirProgram,
    layout: &LayoutCtx,
    method_index: &ActorMethodIndex,
    group_arena_capacity: u64,
    _enqueue_specs: &[(String, u64, u64)],
) -> Result<CodegenProgram, CodegenError> {
    let optimized = crate::mwir_opt::optimize(mwir, Some(flow), layout);
    let mwir = optimized.as_ref().unwrap_or(mwir);
    if block_ids_active() {
        NEXT_BLOCK_ID.with(|c| c.set(0));
    }
    if block_bridge() {
        BLOCK_SPANS.with(|s| s.borrow_mut().clear());
    }
    let mut rodata = RodataPool::new();
    rodata.seed(&mwir.rodata);
    let (child_index, max_children) = compute_group_child_indices(flow)?;
    let gctx = GroupCtx {
        arena_capacity: group_arena_capacity,
        max_children,
        child_index,
    };
    let mut fns = BTreeMap::new();
    let (prepared, conventions) = prepare_sync_fns(mwir, layout, &mut rodata)?;
    for (key, f) in &mwir.fns {
        fns.insert(
            key.clone(),
            emit_fn(
                key,
                f,
                layout,
                &mut rodata,
                &prepared[key],
                conventions.get(key),
            )?,
        );
    }
    for (key, f) in &flow.fns {
        fns.insert(
            key.clone(),
            emit_flowwir_fn(key, f, layout, &mut rodata, method_index, &gctx)?,
        );
    }
    let out = CodegenProgram {
        fns,
        rodata: rodata.entries,
        conventions,
    };
    verify_conventions(&out).map_err(CodegenError::internal)?;
    Ok(out)
}

pub fn codegen_program(
    mwir: &MwirProgram,
    layout: &LayoutCtx,
) -> Result<CodegenProgram, CodegenError> {
    let optimized = crate::mwir_opt::optimize(mwir, None, layout);
    let mwir = optimized.as_ref().unwrap_or(mwir);
    if block_ids_active() {
        NEXT_BLOCK_ID.with(|c| c.set(0));
    }
    if block_bridge() {
        BLOCK_SPANS.with(|s| s.borrow_mut().clear());
    }
    let mut rodata = RodataPool::new();
    rodata.seed(&mwir.rodata);
    let mut fns = BTreeMap::new();
    let (prepared, conventions) = prepare_sync_fns(mwir, layout, &mut rodata)?;
    for (key, f) in &mwir.fns {
        let cf = emit_fn(
            key,
            f,
            layout,
            &mut rodata,
            &prepared[key],
            conventions.get(key),
        )?;
        fns.insert(key.clone(), cf);
    }
    let out = CodegenProgram {
        fns,
        rodata: rodata.entries,
        conventions,
    };
    verify_conventions(&out).map_err(CodegenError::internal)?;
    Ok(out)
}

pub fn dump(program: &CodegenProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (key, f) in &program.fns {
        push_line(
            &mut out,
            1,
            &format!("Fn key={key} frame={} bytes", f.frame_size),
        );
        for (i, ew) in f.code.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("{i:04}: {:08x}  {}", ew.word, ew.text),
            );
        }
    }
    if !program.rodata.is_empty() {
        push_line(&mut out, 1, "Rodata");
        let mut off = 0usize;
        for (i, bytes) in program.rodata.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("{i}: offset={off:#x} {}", render_bytes(bytes)),
            );
            off += bytes.len();
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

pub fn verify_conventions(program: &CodegenProgram) -> Result<(), String> {
    if program.conventions.is_empty() {
        return Ok(());
    }
    for (key, conv) in &program.conventions {
        let Some(f) = program.fns.get(key) else {
            return Err(format!(
                "internal error: fn `{key}` has a convention but no emitted code"
            ));
        };
        let mut actual: regalloc::RegSet = 0;
        for w in &f.code {
            if let Some(d) = w.dst {
                actual |= regalloc::reg_bit(d);
            }
            for &sr in &w.srcs[..w.src_len as usize] {
                actual |= regalloc::reg_bit(sr);
            }
        }
        let mut worst = String::new();
        for r in &f.relocs {
            let Reloc::Call { key: target, .. } = r else {
                continue;
            };
            let reached = match program.conventions.get(target) {
                Some(c) => c.clobbers,
                None => regalloc::ALL_REGS,
            };
            if reached & !conv.clobbers != 0 && worst.is_empty() {
                worst = format!(" (via its call to `{target}`)");
            }
            actual |= reached;
        }
        let missing = actual & !conv.clobbers;
        if missing != 0 {
            return Err(format!(
                "internal error: fn `{key}` was published as clobbering {} but its emitted                  code reaches {}{worst} — every caller that kept a value in {} across a                  call to it has been miscompiled",
                regalloc::render_reg_set(conv.clobbers),
                regalloc::render_reg_set(actual),
                regalloc::render_reg_set(missing),
            ));
        }
    }
    Ok(())
}

pub fn validate(program: &CodegenProgram) -> Result<(), String> {
    let rodata_len: usize = program.rodata.iter().map(Vec::len).sum();
    for (key, f) in &program.fns {
        if f.code.is_empty() {
            return Err(format!(
                "fn `{key}` emitted zero code words (every fn always has at least a \
                 prologue/epilogue)"
            ));
        }
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::Call word {word} is out of range (code has {} \
                             word(s))",
                            f.code.len()
                        ));
                    }
                    let resolvable = program.fns.contains_key(target)
                        || rt_enqueue_actor(target).is_some_and(|a| !a.is_empty())
                        || rt_run_one_glue_target(target)
                        || rt_select_and_run_glue_target(target)
                        || target == "__wrela_rt_run_one"
                        || target == "__wrela_deadline_poll"
                        || target == "__wrela_deadline_scan"
                        || target == "__wrela_rt_checkpoint"
                        || target == "__wrela_vector0";
                    if !resolvable {
                        return Err(format!(
                            "fn `{key}`: Reloc::Call targets `{target}`, which this \
                             `CodegenProgram` never codegen'd and which is not an \
                             `rt_enqueue` / `rt_select_and_run` / `rt_drain` / \
                             `rt_xreply` / `__wrela_*` glue symbol either"
                        ));
                    }
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    if word_adrp + 1 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::Rodata word_adrp {word_adrp} (its paired ADD sits \
                             at +1) is out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                    if *byte_offset >= rodata_len {
                        return Err(format!(
                            "fn `{key}`: Reloc::Rodata byte_offset {byte_offset} is out of range \
                             (rodata is {rodata_len} byte(s))"
                        ));
                    }
                }
                Reloc::RodataAdr { word, byte_offset } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::RodataAdr word {word} is out of range (code has \
                             {} word(s))",
                            f.code.len()
                        ));
                    }
                    if *byte_offset >= rodata_len {
                        return Err(format!(
                            "fn `{key}`: Reloc::RodataAdr byte_offset {byte_offset} is out of \
                             range (rodata is {rodata_len} byte(s))"
                        ));
                    }
                }
                Reloc::AbortFixed { word } | Reloc::AbortVal { word } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::AbortFixed/AbortVal word {word} is out of range \
                             (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::CheckpointService { word } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::CheckpointService word {word} is out of range \
                             (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::TurnFrameAddr { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::TurnFrameAddr word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::TurnsBase { word } | Reloc::TurnStride { word } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::TurnsBase/TurnStride word {word} (a 4-word                              load_imm) is out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::MailboxAddr { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::MailboxAddr word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::RrCursor { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::RrCursor word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::TurnIdImm { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::TurnIdImm word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::GroupArenaBase { word } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::GroupArenaBase word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::IrqVector { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::IrqVector word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::WakePending { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::WakePending word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::RingAddr { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::RingAddr word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::DriverState { word, .. }
                | Reloc::DeviceRegsBase { word, .. }
                | Reloc::PoolBase { word, .. }
                | Reloc::PoolSlot { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::DriverState/DeviceRegsBase/PoolBase/PoolSlot \
                             word {word} (a 4-word load_imm) is out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn emitted_a64_census_specialization_live_counts()
-> std::collections::BTreeMap<&'static str, usize> {
    use std::collections::BTreeMap;
    let mut out = BTreeMap::new();
    out.insert(
        "emit_secondary_sp_install",
        emit_secondary_sp_install(1, 2).len(),
    );
    out.insert("emit_checkpoint_lr_frame", emit_checkpoint_lr_frame().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema;
    use crate::syntax::{ast, lexer, parser};

    fn compile(src: &str) -> (MwirProgram, LayoutCtx) {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let typed = sema::check_typed(&module, "<test>").expect("test source must check");
        let mwir_program = crate::lower::lower_program(&typed).expect("test source must lower");
        let layout = mwir::build_layout_ctx(&module, &Default::default())
            .expect("test source must build a layout ctx");
        (mwir_program, layout)
    }

    #[test]
    fn block_id_pool_exhaustion_is_a_fail_closed_error() {
        set_block_count(true);
        NEXT_BLOCK_ID.with(|c| c.set((crate::rtconfig::BLOCK_POOL_COUNT - 1) as u32));
        let last = alloc_block_id().expect("the final id in the pool must allocate");
        assert_eq!(last as usize, crate::rtconfig::BLOCK_POOL_COUNT - 1);
        let err = alloc_block_id().expect_err("one past the pool must fail");
        assert!(
            err.message.starts_with(FAIL_CLOSED_PREFIX),
            "pool exhaustion must be marked fail-closed, got: {}",
            err.message
        );
        assert!(
            err.message.contains("BLOCK_POOL_COUNT"),
            "the error must name the bound it blew, got: {}",
            err.message
        );
        set_block_count(false);
    }

    #[test]
    fn bridge_mode_alone_does_not_fail_closed_past_the_guest_pool() {
        set_block_count(false);
        set_block_bridge(true);
        NEXT_BLOCK_ID.with(|c| c.set(crate::rtconfig::BLOCK_POOL_COUNT as u32));
        let id = alloc_block_id().expect("bridge mode has no guest array to overflow");
        assert_eq!(id as usize, crate::rtconfig::BLOCK_POOL_COUNT);
        set_block_bridge(false);

        set_block_count(true);
        NEXT_BLOCK_ID.with(|c| c.set(crate::rtconfig::BLOCK_POOL_COUNT as u32));
        assert!(alloc_block_id().is_err(), "emission must still fail closed");
        set_block_count(false);
    }

    #[test]
    fn an_ordinary_codegen_error_is_not_marked_fail_closed() {
        let soft = CodegenError::unimplemented("some shape");
        assert!(
            !soft.message.starts_with(FAIL_CLOSED_PREFIX),
            "unimplemented must remain soft, got: {}",
            soft.message
        );
        let internal = CodegenError::internal("some invariant");
        assert!(
            !internal.message.starts_with(FAIL_CLOSED_PREFIX),
            "producer bugs travel under their own census-tracked prefix, got: {}",
            internal.message
        );
    }

    #[test]
    fn frame_slots_are_assigned_in_temp_order_with_no_packing() {
        let f = MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read)],
            ret: Type::U64,
            temp_types: vec![Type::U8, Type::U64, Type::Bool],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        let frame = build_frame(&f, &layout, 0, 0, 0, &regalloc::Assignment::none(64), true)
            .expect("build_frame");
        assert_eq!(frame.temp_offset, vec![0, 8, 16]);
        assert_eq!(frame.temp_size, vec![8, 8, 8]);
        assert_eq!(frame.self_ptr_off, None);
        assert_eq!(frame.ret_ptr_off, None);
        assert_eq!(frame.lr_off, 24);
        assert_eq!(frame.size, 32);
    }

    #[test]
    fn frame_reserves_self_ptr_and_ret_ptr_slots_when_needed() {
        let f = MwirFn {
            receiver: Some((Temp(0), AccessMode::Mut)),
            params: vec![],
            ret: Type::Named("Point".to_string(), vec![]),
            temp_types: vec![Type::Named("Point".to_string(), vec![])],
            body: vec![Inst::Return { value: None }],
        };
        let mut layout = LayoutCtx::default();
        layout
            .structs
            .insert("Point".to_string(), vec![Type::U64, Type::U64]);
        let frame = build_frame(&f, &layout, 0, 0, 0, &regalloc::Assignment::none(64), true)
            .expect("build_frame");
        assert_eq!(frame.temp_offset, vec![0]);
        assert_eq!(frame.temp_size, vec![16]);
        assert_eq!(frame.self_ptr_off, Some(16));
        assert_eq!(frame.ret_ptr_off, Some(24));
        assert_eq!(frame.lr_off, 32);
        assert_eq!(frame.size, 48);
    }

    #[test]
    fn frame_reserves_the_reply_staging_slot_only_when_sized() {
        let f = MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::U64,
            temp_types: vec![Type::U64],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        let none = build_frame(&f, &layout, 0, 0, 0, &regalloc::Assignment::none(64), true)
            .expect("build_frame");
        assert_eq!(none.reply_stage_off, None);
        assert_eq!(none.lr_off, 8);
        assert_eq!(none.size, 16);
        let staged = build_frame(&f, &layout, 24, 0, 0, &regalloc::Assignment::none(64), true)
            .expect("build_frame");
        assert_eq!(staged.reply_stage_off, Some(8));
        assert_eq!(staged.lr_off, 32);
        assert_eq!(staged.size, 48);
    }

    #[test]
    fn a_frame_over_4095_bytes_fails_closed() {
        let f = MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::Unit,
            temp_types: vec![Type::Array(
                Box::new(Type::U64),
                Box::new(ast::Expr::Int(ast::Span::default(), "600".to_string())),
            )],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        assert!(build_frame(&f, &layout, 0, 0, 0, &regalloc::Assignment::none(64), true).is_err());
    }

    #[test]
    fn an_async_frame_is_bounded_by_imm12_less_the_slot_bias() {
        let f = MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::Unit,
            temp_types: vec![Type::Array(
                Box::new(Type::U64),
                Box::new(ast::Expr::Int(ast::Span::default(), "504".to_string())),
            )],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();

        let sync = build_frame(&f, &layout, 0, 0, 0, &regalloc::Assignment::none(64), true)
            .expect("legal for a sync frame");
        assert_eq!(sync.size, 4048);

        let bias = TURN_RECORD_SIZE as usize;
        assert!(
            sync.size + bias > 4095,
            "this fixture must straddle the boundary to be a regression lock"
        );
        let Err(err) = build_frame(
            &f,
            &layout,
            0,
            0,
            bias,
            &regalloc::Assignment::none(64),
            true,
        ) else {
            panic!("the same frame must be refused once biased past the turn record");
        };
        assert!(
            err.message.contains("4031"),
            "the diagnostic names the biased ceiling: {}",
            err.message
        );

        let smaller = MwirFn {
            temp_types: vec![Type::Array(
                Box::new(Type::U64),
                Box::new(ast::Expr::Int(ast::Span::default(), "500".to_string())),
            )],
            ..f
        };
        let ok = build_frame(
            &smaller,
            &layout,
            0,
            0,
            bias,
            &regalloc::Assignment::none(64),
            true,
        )
        .expect("fits under 4031 with the bias");
        assert!(ok.size + bias <= 4095);
    }

    #[test]
    fn narrow_imm_small_constant_emits_one_word() {
        let mwir = const_return_mwir(42);
        let layout = LayoutCtx::default();

        set_narrow_imm(false);
        let naive = codegen_program(&mwir, &layout).expect("naive");
        set_narrow_imm(true);
        let narrow = codegen_program(&mwir, &layout).expect("narrow");
        set_narrow_imm(false);

        let naive_mov = mov_wide_words(&naive.fns["c"]);
        let narrow_mov = mov_wide_words(&narrow.fns["c"]);
        assert_eq!(
            naive_mov.len(),
            4,
            "naive must stay four words: {naive_mov:?}"
        );
        assert_eq!(
            narrow_mov.len(),
            1,
            "small imm must be one movz: {narrow_mov:?}"
        );
        assert_eq!(narrow_mov[0], encode::enc_movz(X_A, 42, 0, true));
        assert_eq!(materialize_mov_wide(&narrow_mov), 42);
        assert_eq!(
            materialize_mov_wide(&naive_mov),
            materialize_mov_wide(&narrow_mov)
        );
    }

    #[test]
    fn narrow_imm_sparse_skips_zero_movks() {
        let value: u64 = 1u64 << 48;
        let mwir = const_return_mwir(value as i64);
        let layout = LayoutCtx::default();

        set_narrow_imm(true);
        let narrow = codegen_program(&mwir, &layout).expect("narrow");
        set_narrow_imm(false);

        let narrow_mov = mov_wide_words(&narrow.fns["c"]);
        assert_eq!(
            narrow_mov,
            vec![encode::enc_movz(X_A, 1, 48, true)],
            "sparse high half must be a single movz lsl #48"
        );
        assert_eq!(materialize_mov_wide(&narrow_mov), value);

        let value2: u64 = (0xAAu64 << 32) | 0x11;
        let mwir2 = const_return_mwir(value2 as i64);
        set_narrow_imm(true);
        let narrow2 = codegen_program(&mwir2, &layout).expect("narrow2");
        set_narrow_imm(false);
        let mov2 = mov_wide_words(&narrow2.fns["c"]);
        assert_eq!(
            mov2,
            vec![
                encode::enc_movz(X_A, 0x11, 0, true),
                encode::enc_movk(X_A, 0xAA, 32, true),
            ],
            "zero middle half must be skipped"
        );
        assert_eq!(materialize_mov_wide(&mov2), value2);
    }

    #[test]
    fn narrow_imm_bits_match_naive() {
        let layout = LayoutCtx::default();
        let samples: &[i64] = &[
            0,
            1,
            -1,
            0xFFFF,
            0x1_0000,
            0x1_0000_0000,
            (1i64 << 48) | 0x42,
            i64::MIN,
            i64::MAX,
        ];
        for &v in samples {
            let mwir = const_return_mwir(v);
            set_narrow_imm(false);
            let naive = codegen_program(&mwir, &layout).expect("naive");
            set_narrow_imm(true);
            let narrow = codegen_program(&mwir, &layout).expect("narrow");
            set_narrow_imm(false);
            let naive_bits = materialize_mov_wide(&mov_wide_words(&naive.fns["c"]));
            let narrow_bits = materialize_mov_wide(&mov_wide_words(&narrow.fns["c"]));
            assert_eq!(
                naive_bits, narrow_bits,
                "value {v:#x}: naive {naive_bits:#x} != narrow {narrow_bits:#x}"
            );
            assert_eq!(naive_bits, v as u64, "naive must recover {v:#x}");
        }
    }

    fn const_return_mwir(value: i64) -> MwirProgram {
        MwirProgram {
            fns: BTreeMap::from([(
                "c".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![],
                    ret: Type::U64,
                    temp_types: vec![Type::U64],
                    body: vec![
                        Inst::ConstInt {
                            dst: Temp(0),
                            ty: Type::U64,
                            value: value as i128,
                        },
                        Inst::Return {
                            value: Some(Temp(0)),
                        },
                    ],
                },
            )]),
            rodata: vec![],
        }
    }

    fn mov_wide_words(f: &CodegenFn) -> Vec<u32> {
        f.code
            .iter()
            .filter(|ew| ew.rule == CostRule::MovWide)
            .map(|ew| ew.word)
            .collect()
    }

    fn materialize_mov_wide(words: &[u32]) -> u64 {
        let mut val = 0u64;
        for &w in words {
            let imm16 = ((w >> 5) & 0xFFFF) as u64;
            let hw = (w >> 21) & 0b11;
            let shift = hw * 16;
            let opc = (w >> 29) & 0b11;
            match opc {
                0b10 => {
                    val = imm16 << shift;
                }
                0b11 => {
                    let mask = !(0xFFFFu64 << shift);
                    val = (val & mask) | (imm16 << shift);
                }
                other => panic!("unexpected move-wide opc {other:#x} in word {w:#x}"),
            }
        }
        val
    }

    #[test]
    fn emitted_divide_declares_result_and_operands() {
        const SRC: &str = r#"
module examples.cost_div_tags

pub fn q(a: u64, b: u64) -> u64:
    return a / b

pub fn r(a: u64, b: u64) -> u64:
    return a % b
"#;
        let tokens = lexer::lex(SRC).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let mwir_program = crate::lower::lower_program(&typed).expect("lower");
        let prog = codegen_program(&mwir_program, &layout).expect("codegen");

        let mut divides = 0usize;
        let mut msubs = 0usize;
        for f in prog.fns.values() {
            for ew in &f.code {
                match ew.rule {
                    CostRule::Udiv | CostRule::Sdiv => {
                        divides += 1;
                        assert_eq!(
                            ew.dst,
                            Some(X_C),
                            "the divide must declare its quotient register"
                        );
                        assert!(
                            ew.src_slice().contains(&X_A) && ew.src_slice().contains(&X_B),
                            "the divide must declare both operands, got {:?}",
                            ew.src_slice()
                        );
                    }
                    CostRule::Mul => {
                        msubs += 1;
                        assert!(
                            ew.src_slice().contains(&X_A),
                            "msub must declare its accumulator source, got {:?}",
                            ew.src_slice()
                        );
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(divides, 2, "one divide per fn");
        assert_eq!(msubs, 1, "only the `%` lowering emits the msub");

        let table = crate::cost::table::load_default().expect("bench/a76-pi5.toml");
        let place = crate::placement::PlacementTable::default();
        let scored = crate::cost::score_program(&prog, &table, &place).expect("score");
        let q = scored
            .fns
            .iter()
            .find(|f| f.key == "q")
            .expect("fn q scored");
        assert!(
            q.proxy_cycles > table.latency(CostRule::Udiv),
            "the consumer of the quotient must extend past the divide's own {} \
             cycles, got {}",
            table.latency(CostRule::Udiv),
            q.proxy_cycles
        );
    }

    #[test]
    fn narrow_imm_lowers_cost_calls_proxy_rank() {
        use crate::cost::score::score_program;
        use crate::cost::table::load_default;
        let src = include_str!("../../../tests/golden/cost-calls/input.wr");
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let table = load_default().expect("bench/a76-pi5.toml");

        let mwir_program = crate::lower::lower_program(&typed).expect("lower");

        let place = crate::placement::PlacementTable::default();
        set_narrow_imm(false);
        let off_prog = codegen_program(&mwir_program, &layout).expect("codegen off");
        let off = score_program(&off_prog, &table, &place).expect("score off");

        set_narrow_imm(true);
        let on_prog = codegen_program(&mwir_program, &layout).expect("codegen on");
        let on = score_program(&on_prog, &table, &place).expect("score on");
        set_narrow_imm(false);

        assert!(
            on.total_proxy_cycles < off.total_proxy_cycles,
            "NarrowImm-on {} must rank strictly below NarrowImm-off {} on cost-calls",
            on.total_proxy_cycles,
            off.total_proxy_cycles
        );
        let off_mov: usize = off_prog
            .fns
            .values()
            .map(|f| {
                f.code
                    .iter()
                    .filter(|ew| ew.rule == CostRule::MovWide)
                    .count()
            })
            .sum();
        let on_mov: usize = on_prog
            .fns
            .values()
            .map(|f| {
                f.code
                    .iter()
                    .filter(|ew| ew.rule == CostRule::MovWide)
                    .count()
            })
            .sum();
        assert!(
            on_mov < off_mov,
            "NarrowImm must emit fewer mov_wide words ({on_mov} vs {off_mov})"
        );
    }

    #[test]
    fn omit_dmb_strips_barrier_words_from_asm() {
        let mwir = MwirProgram {
            fns: BTreeMap::from([(
                "barrier".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![],
                    ret: Type::Unit,
                    temp_types: vec![],
                    body: vec![
                        Inst::Dmb {
                            option: "ishst".to_string(),
                        },
                        Inst::Dmb {
                            option: "ishld".to_string(),
                        },
                        Inst::Return { value: None },
                    ],
                },
            )]),
            rodata: vec![],
        };
        let layout = LayoutCtx::default();

        set_omit_dmb(false);
        let intact = codegen_program(&mwir, &layout).expect("intact codegen");
        let intact_dump = dump(&intact);
        assert!(
            intact_dump.contains("dmb ishst") && intact_dump.contains("dmb ishld"),
            "intact must emit both barriers:\n{intact_dump}"
        );
        assert!(
            intact_dump.contains(&format!("{:08x}", encode::enc_dmb_ishst())),
            "intact must carry DMB ISHST encoding"
        );
        assert!(
            intact_dump.contains(&format!("{:08x}", encode::enc_dmb_ishld())),
            "intact must carry DMB ISHLD encoding"
        );

        set_omit_dmb(true);
        let mutated = codegen_program(&mwir, &layout).expect("mutated codegen");
        let mutated_dump = dump(&mutated);
        set_omit_dmb(false);
        assert!(
            !mutated_dump.contains("dmb ishst")
                && !mutated_dump.contains("dmb ishld")
                && !mutated_dump.contains(&format!("{:08x}", encode::enc_dmb_ishst()))
                && !mutated_dump.contains(&format!("{:08x}", encode::enc_dmb_ishld())),
            "omit-dmb must strip every DMB word:\n{mutated_dump}"
        );
        let intact_words = intact.fns["barrier"].code.len();
        let mutated_words = mutated.fns["barrier"].code.len();
        assert_eq!(
            intact_words - mutated_words,
            2,
            "exactly two DMB words must disappear under omit-dmb"
        );
    }

    #[test]
    fn block_count_emits_hit_calls_at_leaders() {
        let mwir = MwirProgram {
            fns: BTreeMap::from([(
                "branchy".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![],
                    ret: Type::Unit,
                    temp_types: vec![Type::Bool],
                    body: vec![
                        Inst::ConstBool {
                            dst: Temp(0),
                            value: true,
                        },
                        Inst::JumpIfFalse {
                            cond: Temp(0),
                            target: 3,
                        },
                        Inst::Jump { target: 4 },
                        Inst::ConstBool {
                            dst: Temp(0),
                            value: false,
                        },
                        Inst::Return { value: None },
                    ],
                },
            )]),
            rodata: vec![],
        };
        let layout = LayoutCtx::default();

        set_block_count(false);
        let off = codegen_program(&mwir, &layout).expect("off");
        let off_dump = dump(&off);
        assert!(
            !off_dump.contains("bl <__wrela_block_hit>"),
            "default must not instrument:\n{off_dump}"
        );

        set_block_count(true);
        let on_a = codegen_program(&mwir, &layout).expect("on a");
        let on_b = codegen_program(&mwir, &layout).expect("on b");
        set_block_count(false);
        let on_dump = dump(&on_a);
        assert_eq!(
            dump(&on_a),
            dump(&on_b),
            "block-count emission must be deterministic across two runs"
        );
        let hits = on_dump.matches("bl <__wrela_block_hit>").count();
        assert_eq!(hits, 4, "expected one hit call per leader:\n{on_dump}");
        assert!(
            on_a.fns["branchy"].code.len() > off.fns["branchy"].code.len(),
            "instrumented body must grow"
        );
    }

    #[test]
    fn block_count_instruments_runtime_and_driver_owners() {
        fn two_block_fn() -> MwirFn {
            MwirFn {
                receiver: None,
                params: vec![],
                ret: Type::Unit,
                temp_types: vec![Type::Bool],
                body: vec![
                    Inst::ConstBool {
                        dst: Temp(0),
                        value: true,
                    },
                    Inst::JumpIfFalse {
                        cond: Temp(0),
                        target: 3,
                    },
                    Inst::Jump { target: 3 },
                    Inst::Return { value: None },
                ],
            }
        }

        let keys = [
            "app_fn",
            "core.runtime.helper",
            "Blk.on_turn",
            "__wrela_block_hit",
        ];
        for k in keys {
            let expect = match k {
                "app_fn" => "app",
                "core.runtime.helper" | "__wrela_block_hit" => "runtime",
                _ => "driver",
            };
            assert_eq!(
                crate::cost::owner::classify_owner(k),
                expect,
                "owner fixture for {k} drifted"
            );
        }

        let mwir = MwirProgram {
            fns: keys
                .iter()
                .map(|k| ((*k).to_string(), two_block_fn()))
                .collect(),
            rodata: vec![],
        };
        let layout = LayoutCtx::default();

        set_block_count(true);
        let on = codegen_program(&mwir, &layout).expect("codegen on");
        let ids = block_ids_assigned();
        set_block_count(false);

        for k in ["app_fn", "core.runtime.helper", "Blk.on_turn"] {
            let hits = on.fns[k]
                .code
                .iter()
                .filter(|w| w.text == "bl <__wrela_block_hit>")
                .count();
            assert_eq!(
                hits,
                3,
                "{k} ({}) must be instrumented at every leader under decision 1607",
                crate::cost::owner::classify_owner(k)
            );
        }
        let self_hits = on.fns["__wrela_block_hit"]
            .code
            .iter()
            .filter(|w| w.text == "bl <__wrela_block_hit>")
            .count();
        assert_eq!(
            self_hits, 0,
            "the counter helper must never be instrumented — that is unbounded self-recursion"
        );
        assert_eq!(ids, 9, "one id per instrumented leader, helper excluded");
    }

    #[test]
    fn block_count_id_count_on_boot_actors_cost_stage_is_pinned() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/boot-actors/input.wr");

        set_block_count(true);
        let prog = crate::cost::stage::codegen_cost_stage(&path);
        let ids = block_ids_assigned();
        set_block_count(false);
        let prog = prog.expect("boot-actors cost-stage codegen under --block-count");

        let hits: usize = prog
            .fns
            .values()
            .map(|f| {
                f.code
                    .iter()
                    .filter(|w| w.text == "bl <__wrela_block_hit>")
                    .count()
            })
            .sum();
        assert_eq!(
            hits, ids as usize,
            "every allocated id must emit exactly one hit call"
        );
        assert_eq!(
            ids, 184,
            "boot-actors cost-stage Lane 2 id count moved; re-measure and cite the new number \
             (plans/M20.md item B)"
        );
        assert!(
            (ids as usize) < crate::rtconfig::BLOCK_POOL_COUNT,
            "cost-stage id count {ids} must stay under BLOCK_POOL_COUNT {}",
            crate::rtconfig::BLOCK_POOL_COUNT
        );
    }

    #[test]
    fn mwir_block_leaders_marks_targets_and_fallthrough() {
        let body = vec![
            Inst::ConstBool {
                dst: Temp(0),
                value: true,
            },
            Inst::JumpIfFalse {
                cond: Temp(0),
                target: 3,
            },
            Inst::Jump { target: 4 },
            Inst::ConstBool {
                dst: Temp(0),
                value: false,
            },
            Inst::Return { value: None },
        ];
        assert_eq!(
            mwir_block_leaders(&body),
            vec![true, false, true, true, true]
        );
    }

    #[test]
    fn add_emission_records_alu_rule_and_regs() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_cost_add\n\npub fn add(a: i64, b: i64) -> i64:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let adds = f
            .code
            .iter()
            .find(|ew| ew.text.starts_with("adds "))
            .expect("expected adds in add body");
        assert_eq!(adds.rule, CostRule::Alu);
        assert_eq!(adds.dst, Some(X_C));
        assert_eq!(adds.src_slice(), &[X_A, X_B]);
    }

    #[test]
    fn sync_frame_load_store_tagged_stack() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_memref_stack\n\npub fn answer() -> u64:\n    return 42\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["answer"];
        let mem_ops: Vec<&EmittedWord> = f
            .code
            .iter()
            .filter(|ew| matches!(ew.rule, CostRule::Load | CostRule::Store))
            .collect();
        assert!(!mem_ops.is_empty());
        for ew in mem_ops {
            let mem = ew.mem.expect("load/store must be tagged");
            assert_eq!(
                mem.class,
                crate::cost::MemClass::Stack,
                "sp-relative {} should be Stack: {}",
                ew.rule.as_str(),
                ew.text
            );
        }
    }

    #[test]
    fn adrp_has_no_memref() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_memref_adrp\n\npub fn add(a: u8, b: u8) -> u8:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let adrps: Vec<&EmittedWord> = f
            .code
            .iter()
            .filter(|ew| ew.rule == CostRule::Adrp)
            .collect();
        assert!(!adrps.is_empty(), "expected adrp in abort stubs");
        for ew in &adrps {
            assert_eq!(ew.mem, None, "adrp must not carry MemRef: {}", ew.text);
        }
    }

    #[test]
    fn adr_addressing_replaces_every_adrp_add_pair_with_one_adr() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_adr_rodata\n\npub fn add(a: u8, b: u8) -> u8:\n    return a + b\n",
        );

        set_adr_addressing(false);
        let pair = codegen_program(&mwir_program, &layout).expect("adrp+add side");
        set_adr_addressing(true);
        let adr = codegen_program(&mwir_program, &layout).expect("adr side");
        set_adr_addressing(false);

        let pf = &pair.fns["add"];
        let af = &adr.fns["add"];

        let sites = pf
            .relocs
            .iter()
            .filter(|r| matches!(r, Reloc::Rodata { .. }))
            .count();
        assert!(sites > 0, "this fixture must emit rodata references at all");
        assert_eq!(
            af.relocs
                .iter()
                .filter(|r| matches!(r, Reloc::RodataAdr { .. }))
                .count(),
            sites,
            "every Reloc::Rodata must become a Reloc::RodataAdr"
        );
        assert!(
            !af.relocs.iter().any(|r| matches!(r, Reloc::Rodata { .. })),
            "no ADRP+ADD reloc may survive the substitution"
        );

        assert_eq!(
            pf.code.len() - af.code.len(),
            sites,
            "the ADR form must be exactly one word shorter per site"
        );
        for r in &af.relocs {
            let Reloc::RodataAdr { word, .. } = r else {
                continue;
            };
            let w = af.code[*word].word;
            assert_eq!(
                w & 0x9F00_0000,
                0x1000_0000,
                "word {word} must be an ADR, not an ADRP: {:#010x} / {}",
                w,
                af.code[*word].text
            );
            assert_eq!(
                af.code[*word].rule,
                CostRule::Adrp,
                "ADR keeps the PC-relative rule class — it is the same \
                 encoding family and the same A76 port; no cost row moves"
            );
        }
    }

    fn words_under(src: &str, key: &str, opts: &[crate::opts::OptId]) -> Vec<EmittedWord> {
        crate::opts::apply_opts(opts);
        let (mwir_program, layout) = compile(src);
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let out = program.fns[key].code.clone();
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        out
    }

    fn texts(words: &[EmittedWord]) -> Vec<String> {
        words.iter().map(|w| w.text.clone()).collect()
    }

    #[test]
    fn item_c3_narrow_to_width_becomes_one_bitfield_extract() {
        use crate::opts::OptId;
        const SRC: &str = "module examples.item_c3\n\n\
             pub fn wrap_u8(a: u8, b: u8) -> u8:\n    return a +% b\n\n\
             pub fn wrap_i16(a: i16, b: i16) -> i16:\n    return a +% b\n";

        for (key, want, gone) in [
            ("wrap_u8", "ubfx x11, x11, #0, #8", "lsr x11, x11, #56"),
            ("wrap_i16", "sbfx x11, x11, #0, #16", "asr x11, x11, #48"),
        ] {
            let off = texts(&words_under(SRC, key, &[]));
            let on = texts(&words_under(SRC, key, &[OptId::BfxNarrow]));

            assert!(
                off.iter().any(|t| t == gone) && off.iter().any(|t| t.starts_with("lsl x11")),
                "{key}: the baseline must be the shift pair, got {off:?}"
            );
            assert!(
                on.iter().any(|t| t == want),
                "{key}: expected `{want}`, got {on:?}"
            );
            assert!(
                !on.iter().any(|t| t == gone)
                    && !on.iter().any(|t| t.starts_with("lsl x11, x11, #")),
                "{key}: the shift pair must be gone, got {on:?}"
            );
            assert_eq!(
                on.len() + 1,
                off.len(),
                "{key}: the substitution must remove exactly one word"
            );
        }
    }

    #[test]
    fn adr_addressing_is_off_under_dev() {
        crate::opts::apply_mode(crate::opts::CompileMode::Dev);
        assert!(!adr_addressing());
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        assert!(adr_addressing());
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
    }

    #[test]
    fn item_c2_narrow_range_check_becomes_one_masked_test() {
        use crate::opts::OptId;
        const SRC: &str = "module examples.item_c2\n\n\
             pub fn add_u32(a: u32, b: u32) -> u32:\n    return a + b\n\n\
             pub fn add_i16(a: i16, b: i16) -> i16:\n    return a + b\n";

        let off = words_under(SRC, "add_u32", &[]);
        let on = words_under(SRC, "add_u32", &[OptId::MaskCheck]);
        let on_t = texts(&on);
        assert!(
            on_t.iter().any(|t| t == "tst x11, #0xffffffff00000000"),
            "expected the high-mask TST, got {on_t:?}"
        );
        assert!(
            !on_t.iter().any(|t| t.starts_with("cmp x11, x12")),
            "the two constant compares must be gone, got {on_t:?}"
        );
        assert_eq!(!((1u64 << 32) - 1), 0xFFFF_FFFF_0000_0000);

        let on_s = texts(&words_under(SRC, "add_i16", &[OptId::MaskCheck]));
        assert!(
            on_s.iter().any(|t| t == "sbfx x12, x11, #0, #16"),
            "expected the SBFX range test, got {on_s:?}"
        );

        let aborts = |w: &[EmittedWord]| w.iter().filter(|e| e.rule == CostRule::Abort).count();
        assert_eq!(aborts(&off), 2, "the baseline had one abort per direction");
        assert_eq!(aborts(&on), 1, "the masked form needs only one");
        assert!(
            on_t.iter().any(|t| t.contains("__wrela_abort")),
            "the overflow abort must survive, got {on_t:?}"
        );
    }

    #[test]
    fn item_c5_one_word_immediates() {
        use crate::opts::OptId;
        const SRC: &str = "module examples.item_c5\n\n\
             pub fn div(a: i32, b: i32) -> i32:\n    return a / b\n";

        let ni = texts(&words_under(SRC, "div", &[OptId::NarrowImm]));
        let c5 = texts(&words_under(
            SRC,
            "div",
            &[OptId::NarrowImm, OptId::WideImmForms],
        ));

        assert_eq!(
            ni.iter()
                .filter(|t| t.starts_with("movz x13, #0xffff"))
                .count(),
            1,
            "NarrowImm alone must still open the -1 chain with a MOVZ: {ni:?}"
        );
        assert_eq!(
            ni.iter().filter(|t| t.starts_with("movk x13,")).count(),
            3,
            "NarrowImm alone must still need three MOVKs for -1: {ni:?}"
        );
        assert!(
            c5.iter().any(|t| t == "movn x13, #0x0"),
            "expected `movn x13, #0x0` for -1, got {c5:?}"
        );
        assert!(
            !c5.iter().any(|t| t.starts_with("movk x13,")),
            "the -1 MOVK chain must be gone, got {c5:?}"
        );
        assert!(
            c5.iter().any(|t| t == "mov x12, #0xffffffff80000000"),
            "expected the bitmask-immediate MOV for i32::MIN, got {c5:?}"
        );
        assert_eq!(
            c5.len() + 5,
            ni.len(),
            "-1 goes 4 words -> 1 and i32::MIN goes 3 -> 1"
        );
    }

    #[test]
    fn item_c1_only_narrow_wrapping_multiplies_take_the_w_form() {
        const SRC: &str = "module examples.item_c1\n\n\
             pub fn w32(a: u32, b: u32) -> u32:\n    return a *% b\n\n\
             pub fn w8(a: u8, b: u8) -> u8:\n    return a *% b\n\n\
             pub fn wi32(a: i32, b: i32) -> i32:\n    return a *% b\n\n\
             pub fn w64(a: u64, b: u64) -> u64:\n    return a *% b\n\n\
             pub fn c32(a: u32, b: u32) -> u32:\n    return a * b\n";

        for key in ["w32", "w8", "wi32"] {
            let w = words_under(SRC, key, crate::opts::RELEASE_OPTS);
            let muls: Vec<&EmittedWord> = w
                .iter()
                .filter(|e| matches!(e.rule, CostRule::Mul | CostRule::MulW))
                .collect();
            assert_eq!(muls.len(), 1, "{key}: exactly one multiply");
            assert_eq!(
                muls[0].rule,
                CostRule::MulW,
                "{key}: a wrapping multiply at <= 32 bits must be W-form, got `{}`",
                muls[0].text
            );
            assert_eq!(muls[0].word >> 31, 0, "{key}: sf bit must be 0");
            assert!(
                muls[0].text.starts_with("mul w"),
                "{key}: the dump must print W registers, got `{}`",
                muls[0].text
            );
        }

        for (key, why) in [
            ("w64", "64 bits is not a narrow type"),
            (
                "c32",
                "a checked multiply's overflow test reads the high half",
            ),
        ] {
            let w = words_under(SRC, key, crate::opts::RELEASE_OPTS);
            let muls: Vec<&EmittedWord> = w
                .iter()
                .filter(|e| matches!(e.rule, CostRule::Mul | CostRule::MulW))
                .collect();
            assert!(!muls.is_empty(), "{key}: expected a multiply");
            for m in muls {
                assert_eq!(
                    m.rule,
                    CostRule::Mul,
                    "{key} must stay X-form — {why}; got `{}`",
                    m.text
                );
                assert_eq!(m.word >> 31, 1, "{key}: sf bit must be 1");
            }
        }
    }

    #[test]
    fn memref_for_base_imm_non_sp_is_cold_in_codegen_helpers() {
        assert_eq!(
            MemRef::for_base_imm(X_FRAME, 64).class,
            crate::cost::MemClass::Cold
        );
        assert_eq!(MemRef::for_base_imm(X_SP, 16), MemRef::stack(16));
    }

    #[test]
    fn unknown_load_via_push_gets_unique_cold() {
        let u0 = MemRef::cold_unique(0);
        let u1 = MemRef::cold_unique(1);
        assert_eq!(u0.class, crate::cost::MemClass::Cold);
        assert_ne!(u0.key, u1.key);
        assert_ne!(u0.key & (1u64 << 63), 0);
        let stable = MemRef::for_base_imm(X_A, 0);
        assert_eq!(stable.class, crate::cost::MemClass::Cold);
        assert_eq!(stable.key & (1u64 << 63), 0);
    }

    #[test]
    fn push_shape_call_requires_x0_dst() {
        check_push_shape(CostRule::Call, Some(0), &[], None);
        check_push_shape(CostRule::Call, Some(0), &[1, 2], None);
    }

    #[test]
    #[should_panic(expected = "Call must declare dst=Some(0)")]
    fn push_shape_call_without_x0_dst_fails() {
        check_push_shape(CostRule::Call, None, &[], None);
    }

    #[test]
    #[should_panic(expected = "Call must declare dst=Some(0)")]
    fn push_shape_call_wrong_dst_fails() {
        check_push_shape(CostRule::Call, Some(1), &[], None);
    }

    #[test]
    fn push_shape_load_known_addr_needs_src() {
        check_push_shape(
            CostRule::Load,
            Some(0),
            &[MEM_SP_REG],
            Some(&MemRef::stack(8)),
        );
        check_push_shape(
            CostRule::Load,
            Some(0),
            &[X_A],
            Some(&MemRef::for_base_imm(X_A, 0)),
        );
    }

    #[test]
    #[should_panic(expected = "Load with known address")]
    fn push_shape_load_known_addr_without_src_fails() {
        check_push_shape(CostRule::Load, Some(0), &[], Some(&MemRef::stack(8)));
    }

    #[test]
    fn push_shape_load_unique_cold_empty_srcs_ok() {
        check_push_shape(CostRule::Load, Some(0), &[], Some(&MemRef::cold_unique(0)));
    }

    #[test]
    fn push_shape_store_nonunique_requires_base_in_srcs() {
        check_push_shape(
            CostRule::Store,
            None,
            &[0, MEM_SP_REG],
            Some(&MemRef::stack(16)),
        );
        check_push_shape(
            CostRule::Store,
            None,
            &[1, X_FRAME],
            Some(&MemRef::for_base_imm(X_FRAME, 64)),
        );
    }

    #[test]
    #[should_panic(expected = "base reg")]
    fn push_shape_store_nonunique_missing_base_fails() {
        check_push_shape(CostRule::Store, None, &[0], Some(&MemRef::stack(8)));
    }

    #[test]
    fn push_shape_store_unique_cold_exempt_from_base() {
        check_push_shape(CostRule::Store, None, &[0], Some(&MemRef::cold_unique(3)));
    }

    #[test]
    fn push_shape_untagged_load_store_helpers_unique() {
        assert!(memref_is_unique_cold(&MemRef::cold_unique(0)));
        assert!(!memref_is_unique_cold(&MemRef::stack(0)));
        assert!(!memref_is_unique_cold(&MemRef::for_base_imm(X_A, 0)));
        assert_eq!(memref_nonunique_base(&MemRef::stack(24)), Some(MEM_SP_REG));
        assert_eq!(
            memref_nonunique_base(&MemRef::for_base_imm(X_FRAME, 8)),
            Some(X_FRAME)
        );
        assert_eq!(memref_nonunique_base(&MemRef::cold_unique(1)), None);
    }

    #[test]
    fn a_fn_returning_a_constant_compiles_to_the_expected_word_sequence() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_const_test\n\npub fn answer() -> u64:\n    return 42\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["answer"];
        assert_eq!(f.frame_size, 16);
        let words: Vec<u32> = f.code.iter().map(|ew| ew.word).collect();
        assert_eq!(
            words,
            vec![
                encode::enc_sub_imm(X_SP, X_SP, 16, true),
                encode::enc_str_x_imm(X_LR, X_SP, 8),
                encode::enc_movz(X_A, 0x2a, 0, true),
                encode::enc_movk(X_A, 0, 16, true),
                encode::enc_movk(X_A, 0, 32, true),
                encode::enc_movk(X_A, 0, 48, true),
                encode::enc_str_x_imm(X_A, X_SP, 0),
                encode::enc_ldr_x_imm(0, X_SP, 0),
                encode::enc_b(8),
                encode::enc_b(4),
                encode::enc_ldr_x_imm(X_LR, X_SP, 8),
                encode::enc_add_imm(X_SP, X_SP, 16, true),
                encode::enc_ret(X_LR),
            ]
        );
        assert!(f.relocs.is_empty());
    }

    #[test]
    fn nested_calls_emit_symbolic_call_relocs_pointing_at_the_right_words() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_call_test\n\npub fn add_one(x: u64) -> u64:\n    return x + 1\n\npub fn combo(x: u64) -> u64:\n    return add_one(x)\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let combo = &program.fns["combo"];
        let call_relocs: Vec<&Reloc> = combo
            .relocs
            .iter()
            .filter(|r| matches!(r, Reloc::Call { .. }))
            .collect();
        assert_eq!(call_relocs.len(), 1);
        match call_relocs[0] {
            Reloc::Call { word, key } => {
                assert_eq!(key, "add_one");
                let ew = &combo.code[*word];
                assert_eq!(ew.word, encode::enc_bl(0));
                assert_eq!(ew.text, "bl <add_one>");
                assert_eq!(ew.rule, CostRule::Call);
                assert_eq!(ew.dst, Some(0));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn narrow_checked_add_bounds_checks_against_the_target_type() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_narrow\n\npub fn add(a: u32, b: u32) -> u32:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("add x")));
        assert_eq!(mnems.iter().filter(|m| m.starts_with("cmp")).count(), 2);
        assert!(mnems.iter().any(|m| m.starts_with("b.ge")));
        assert!(mnems.iter().any(|m| m.starts_with("b.le")));
        assert_eq!(
            mnems.iter().filter(|m| **m == "bl <__wrela_abort>").count(),
            2
        );
    }

    #[test]
    fn wide_checked_add_uses_flags_not_a_bounds_compare() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_wide_signed\n\npub fn add(a: i64, b: i64) -> i64:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("adds x")));
        assert!(mnems.iter().any(|m| m.starts_with("b.vc")));
        assert!(!mnems.iter().any(|m| m.starts_with("cmp")));
    }

    #[test]
    fn wide_unsigned_checked_sub_uses_the_carry_clear_condition() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_wide_unsigned\n\npub fn sub(a: u64, b: u64) -> u64:\n    return a - b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["sub"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("subs x")));
        assert!(mnems.iter().any(|m| m.starts_with("b.cs")));
    }

    #[test]
    fn wide_checked_mul_uses_smulh_and_a_sign_extension_compare() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_wide_mul\n\npub fn mul(a: i64, b: i64) -> i64:\n    return a * b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["mul"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("mul x")));
        assert!(mnems.iter().any(|m| m.starts_with("smulh")));
        assert!(mnems.iter().any(|m| m.starts_with("asr")));
        assert!(mnems.iter().any(|m| m.starts_with("b.eq")));
    }

    #[test]
    fn signed_div_checks_min_over_neg_one_before_dividing() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_div\n\npub fn div(a: i32, b: i32) -> i32:\n    return a / b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["div"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("sdiv")));
        assert_eq!(
            mnems.iter().filter(|m| **m == "bl <__wrela_abort>").count(),
            2
        );
    }

    #[test]
    fn unsigned_div_never_checks_min_over_neg_one() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_udiv\n\npub fn div(a: u32, b: u32) -> u32:\n    return a / b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["div"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("udiv")));
        assert_eq!(
            mnems.iter().filter(|m| **m == "bl <__wrela_abort>").count(),
            1
        );
    }

    #[test]
    fn shift_range_check_is_one_unsigned_compare() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_shift\n\npub fn shl(a: u32, n: u32) -> u32:\n    return a << n\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["shl"];
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("b.cc")));
        assert!(mnems.iter().any(|m| m.starts_with("cbz")));
        assert!(mnems.iter().any(|m| m.starts_with("lsl x")));
    }

    #[test]
    fn rodata_pool_dedups_identical_bytes_by_content() {
        let mut pool = RodataPool::new();
        let a = pool.intern(b"hello".to_vec());
        let b = pool.intern(b"world".to_vec());
        let c = pool.intern(b"hello".to_vec());
        assert_eq!(a, c);
        assert_ne!(a, b);
        assert_eq!(pool.entries.len(), 2);
        assert_eq!(pool.byte_offset(0), 0);
        assert_eq!(pool.byte_offset(1), 5);
    }

    #[test]
    fn identical_abort_messages_across_fns_share_one_rodata_entry() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_rodata_dedup\n\npub fn add1(a: u32, b: u32) -> u32:\n    return a + b\n\npub fn add2(a: u32, b: u32) -> u32:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        assert_eq!(program.rodata.len(), 1);
        assert_eq!(program.rodata[0], b"arithmetic overflow in `+`");
    }

    #[test]
    fn codegen_is_deterministic_across_repeated_runs() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_determinism\n\npub fn add(a: u32, b: u32) -> u32:\n    return a + b\n",
        );
        let p1 = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let p2 = codegen_program(&mwir_program, &layout).expect("codegen_program");
        assert_eq!(p1, p2);
    }

    #[test]
    fn a_float_typed_constant_fails_closed() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_float_fails_closed\n\npub fn half() -> f64:\n    return 0.5\n",
        );
        let err = codegen_program(&mwir_program, &layout).unwrap_err();
        assert!(err.message.contains("floating-point"));
    }

    #[test]
    fn more_than_eight_call_arguments_fails_closed() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_too_many_args\n\npub fn nine(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64, i: u64) -> u64:\n    return a\n\npub fn caller() -> u64:\n    return nine(1, 2, 3, 4, 5, 6, 7, 8, 9)\n",
        );
        let err = codegen_program(&mwir_program, &layout).unwrap_err();
        assert!(err.message.contains("8 call arguments"));
    }

    #[test]
    fn validate_accepts_a_real_multi_fn_program() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_validate_ok\n\npub fn add_one(x: u64) -> u64:\n    return x + 1\n\npub fn use_it(x: u64) -> u64:\n    return add_one(x)\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        assert!(program.fns.values().any(|f| !f.relocs.is_empty()));
        validate(&program).expect("a real codegen'd program must validate");
    }

    #[test]
    fn validate_rejects_empty_code() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:empty".to_string(),
            CodegenFn {
                frame_size: 16,
                code: Vec::new(),
                relocs: Vec::new(),
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(err.contains("emitted zero code words"), "{err}");
    }

    #[test]
    fn validate_rejects_a_call_reloc_to_an_unknown_fn() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:caller".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![EmittedWord::new(0, String::new(), CostRule::Alu, None, &[])],
                relocs: vec![Reloc::Call {
                    word: 0,
                    key: "fn:ghost".to_string(),
                }],
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(err.contains("never codegen'd"), "{err}");
    }

    #[test]
    fn validate_rejects_a_call_reloc_word_out_of_range() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:only".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![EmittedWord::new(0, String::new(), CostRule::Alu, None, &[])],
                relocs: vec![Reloc::Call {
                    word: 5,
                    key: "fn:only".to_string(),
                }],
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(err.contains("Reloc::Call word 5 is out of range"), "{err}");
    }

    #[test]
    fn validate_rejects_a_rodata_reloc_byte_offset_out_of_range() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:only".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![
                    EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]),
                    EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]),
                ],
                relocs: vec![Reloc::Rodata {
                    word_adrp: 0,
                    byte_offset: 100,
                }],
            },
        );
        program.rodata.push(b"hi".to_vec());
        let err = validate(&program).unwrap_err();
        assert!(
            err.contains("Reloc::Rodata byte_offset 100 is out of range"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_an_abort_reloc_word_out_of_range() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:only".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![EmittedWord::new(0, String::new(), CostRule::Alu, None, &[])],
                relocs: vec![Reloc::AbortFixed { word: 3 }],
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(
            err.contains("Reloc::AbortFixed/AbortVal word 3 is out of range"),
            "{err}"
        );
    }

    #[test]
    fn mmio_access_width_fail_closed_arms() {
        let signed = mmio_access_width(&Type::I32, 0).expect_err("signed");
        assert!(
            signed.message.contains("unsigned") && signed.message.contains("sign-extending"),
            "{}",
            signed.message
        );

        let far = mmio_access_width(&Type::U32, 0x10000).expect_err("far");
        assert!(
            far.message.contains("0x10000") && far.message.contains("unsigned-immediate"),
            "{}",
            far.message
        );

        let misaligned = mmio_access_width(&Type::U32, 1).expect_err("misaligned");
        assert!(
            misaligned.message.contains("internal error:")
                && misaligned.message.contains("not 4-byte aligned")
                && misaligned.message.contains("check_layouts"),
            "{}",
            misaligned.message
        );
    }
}

#[cfg(test)]
mod synthetic_symbol_tests {
    use super::*;

    #[test]
    fn synthesized_symbols_are_unrepresentable_as_source_keys() {
        let sym = rt_enqueue_symbol("Doubler");
        assert!(symbol_is_synthetic(&sym), "{sym} must be synthetic");
        assert_eq!(rt_enqueue_actor(&sym), Some("Doubler"));
        for plausible in [
            "rt_enqueue_Doubler",
            "__rt_enqueue_Doubler",
            "Doubler",
            "A.rt_enqueue",
        ] {
            assert!(
                !symbol_is_synthetic(plausible),
                "{plausible} is source-shaped"
            );
            assert_ne!(plausible, sym);
        }
        assert!(symbol_is_synthetic(&rt_run_one_symbol(0)));
        assert!(symbol_is_synthetic(&rt_select_and_run_symbol("Store")));
        assert!(symbol_is_synthetic(&rt_child_poll_symbol("child")));
        assert!(symbol_is_synthetic(&rt_drain_symbol(1)));
        assert!(symbol_is_synthetic(&rt_xreply_symbol(0, 1)));
        assert!(symbol_is_synthetic(&rt_xsend_symbol(0, "Actor")));
        assert!(symbol_is_synthetic(&rt_secondary_core_entry_symbol(1)));
        assert!(symbol_is_synthetic(&rt_boot_init_symbol()));
    }
}

#[cfg(test)]
mod rt_cross_core_tests {
    use super::*;

    #[test]
    fn xreply_cores_parse_and_secondary_still_pins_run_one() {
        assert_eq!(rt_xreply_cores(&rt_xreply_symbol(1, 0)), Some((1, 0)));
        assert_eq!(rt_xreply_cores("rt_enqueue Actor"), None);

        let sp = emit_secondary_sp_install(1, 2);
        assert_eq!(sp.len(), 5);
    }
}

#[cfg(test)]
mod regalloc_tests {
    use super::*;
    use crate::opts::{CompileMode, OptId, apply_mode, apply_opts};
    use crate::sema;
    use crate::syntax::{lexer, parser};

    const WITHOUT: &[OptId] = &[OptId::NarrowImm];
    const WITH: &[OptId] = &[OptId::NarrowImm, OptId::RegAlloc];

    pub(super) fn emit(src: &str, opts: &[OptId]) -> CodegenProgram {
        apply_opts(opts);
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let mwir = crate::lower::lower_program(&typed).expect("lower");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout ctx");
        let prog = codegen_program(&mwir, &layout).expect("codegen");
        apply_mode(CompileMode::Release);
        prog
    }

    pub(super) fn rule_count(prog: &CodegenProgram, key: &str, rule: CostRule) -> usize {
        prog.fns
            .get(key)
            .unwrap_or_else(|| panic!("no fn `{key}` in {:?}", prog.fns.keys().collect::<Vec<_>>()))
            .code
            .iter()
            .filter(|w| w.rule == rule)
            .count()
    }

    pub(super) fn frame_of(prog: &CodegenProgram, key: &str) -> usize {
        prog.fns.get(key).expect("fn present").frame_size
    }

    const TWICE: &str = r#"
module examples.regalloc_twice

pub fn used_twice(a: u64) -> u64:
    x: u64 = a +% 1
    return x +% x
"#;

    #[test]
    fn a_value_used_twice_stops_round_tripping_through_the_frame() {
        let before = emit(TWICE, WITHOUT);
        let after = emit(TWICE, WITH);

        let (lb, sb) = (
            rule_count(&before, "used_twice", CostRule::Load),
            rule_count(&before, "used_twice", CostRule::Store),
        );
        let (la, sa) = (
            rule_count(&after, "used_twice", CostRule::Load),
            rule_count(&after, "used_twice", CostRule::Store),
        );
        assert!(
            lb >= 4 && sb >= 3,
            "the spill-everything baseline must really round-trip: {lb} loads, {sb} stores"
        );
        assert!(
            la < lb && sa < sb,
            "residency must delete memory traffic: {lb} -> {la} loads, {sb} -> {sa} stores"
        );
        assert!(
            lb - la >= 2,
            "both reloads of the twice-read value must go: {lb} -> {la}"
        );
    }

    #[test]
    fn the_frame_shrinks_on_asm_loop_sum_array() {
        let read = |case: &str| {
            std::fs::read_to_string(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../tests/golden/{case}/input.wr")),
            )
            .unwrap_or_else(|e| panic!("the {case} golden input: {e}"))
        };
        let loop_src = read("asm-loop");
        assert_eq!(frame_of(&emit(&loop_src, WITHOUT), "sum_array"), 160);
        assert_eq!(frame_of(&emit(&loop_src, WITH), "sum_array"), 80);

        let arith_src = read("asm-arith");
        assert_eq!(frame_of(&emit(&arith_src, WITHOUT), "checked_add"), 32);
        assert_eq!(
            frame_of(&emit(&arith_src, WITH), "checked_add"),
            16,
            "decision 1765 refused these single-read temps; item I promotes \
             them, because coalescing makes the copy free — 32 -> 16 rather \
             than to zero, because not every value here fits (decision 1904)"
        );
    }

    #[test]
    fn more_live_values_than_registers_still_spills_correctly() {
        let mut src =
            String::from("module examples.regalloc_pressure\n\npub fn wide(a: u64) -> u64:\n");
        let n = regalloc::POOL.len() * 3;
        for i in 0..n {
            src.push_str(&format!("    v{i}: u64 = a +% {i}\n"));
        }
        src.push_str("    total: u64 = 0\n");
        for i in 0..n {
            src.push_str(&format!("    total = total +% v{i}\n"));
        }
        src.push_str("    return total\n");

        let after = emit(&src, WITH);
        let before = emit(&src, WITHOUT);
        assert!(
            rule_count(&after, "wide", CostRule::Store) > 1,
            "the surplus must still be spilled, not silently dropped"
        );
        assert!(
            frame_of(&after, "wide") < frame_of(&before, "wide"),
            "the pool's worth of temps must still leave the frame"
        );
        assert!(
            rule_count(&after, "wide", CostRule::Load)
                < rule_count(&before, "wide", CostRule::Load),
            "and the resident ones must stop being reloaded"
        );
    }

    #[test]
    fn dev_is_byte_for_byte_the_naive_frame() {
        let with_opt_off = emit(TWICE, WITHOUT);
        apply_mode(CompileMode::Dev);
        let dev = {
            let tokens = lexer::lex(TWICE).expect("lex");
            let module = parser::parse(tokens).expect("parse");
            let typed = sema::check_typed(&module, "<test>").expect("check");
            let mwir = crate::lower::lower_program(&typed).expect("lower");
            let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout ctx");
            codegen_program(&mwir, &layout).expect("codegen")
        };
        apply_mode(CompileMode::Release);
        assert_eq!(
            frame_of(&dev, "used_twice"),
            frame_of(&with_opt_off, "used_twice"),
            "`dev` and `release`-minus-RegAlloc must agree on the frame"
        );
        assert!(
            dev.fns["used_twice"]
                .code
                .iter()
                .any(|w| w.rule == CostRule::Load),
            "`dev` must still round-trip through the frame"
        );
    }

    #[test]
    fn no_emitted_word_names_a_pool_register_outside_the_pool() {
        let prog = emit(TWICE, WITH);
        for (key, f) in &prog.fns {
            for w in &f.code {
                let mut regs: Vec<u8> = w.srcs[..w.src_len as usize].to_vec();
                if let Some(d) = w.dst {
                    regs.push(d);
                }
                for r in regs {
                    assert!(
                        !(18..=18).contains(&r) && !(28..=29).contains(&r),
                        "fn `{key}`: word `{}` names reserved x{r}",
                        w.text
                    );
                }
            }
        }
    }

    #[test]
    fn a_naive_assignment_leaves_no_temp_resident() {
        let naive = regalloc::Assignment::none(3);
        let f = MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: crate::sema::types::Type::Unit,
            temp_types: vec![
                crate::sema::types::Type::U64,
                crate::sema::types::Type::U64,
                crate::sema::types::Type::U64,
            ],
            body: Vec::new(),
        };
        let layout = LayoutCtx::default();
        let frame = build_frame(&f, &layout, 0, 0, 0, &naive, true).expect("naive frame");
        assert!(frame.virt_to_reg.is_empty());
        for t in 0..3 {
            assert!(
                frame.off(Temp(t)) < VIRT_SLOT_BASE,
                "temp {t} must have a real frame offset"
            );
            assert_eq!(frame.reg_at(frame.off(Temp(t))), None);
        }
    }
}

#[cfg(test)]
mod item_f_tests {
    use super::regalloc_tests::{emit, frame_of, rule_count};
    use super::*;
    use crate::opts::{OptId, RELEASE_OPTS};

    #[rustfmt::skip]
    const E: &[OptId] = &[
        OptId::NarrowImm,
        OptId::AdrAddressing,
        OptId::BfxNarrow,
        OptId::MaskCheck,
        OptId::WideImmForms,
        OptId::RegAlloc,
    ];

    fn release_without_item_j() -> Vec<OptId> {
        let mut v: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| !matches!(o, OptId::ConstProp | OptId::Gvn | OptId::Dce))
            .collect();
        v.push(OptId::Frameless);
        v
    }

    fn mnems<'a>(prog: &'a CodegenProgram, key: &str) -> Vec<&'a str> {
        prog.fns
            .get(key)
            .unwrap_or_else(|| panic!("no fn `{key}` in {:?}", prog.fns.keys().collect::<Vec<_>>()))
            .code
            .iter()
            .map(|w| w.text.as_str())
            .collect()
    }

    const LEAF: &str = r#"
module examples.item_f_leaf

pub fn blend(a: u64, b: u64) -> u64:
    x: u64 = a +% b
    return (x +% x) +% (x *% 2)
"#;

    #[test]
    fn a_leaf_does_not_save_the_link_register() {
        let before = emit(LEAF, E);
        let after = emit(LEAF, &release_without_item_j());
        let b = mnems(&before, "blend");
        assert!(
            b.iter().any(|t| t.starts_with("str lr")),
            "item E saved x30 here; if not, the oracle is vacuous: {b:?}"
        );
        let a = mnems(&after, "blend");
        assert!(
            !a.iter().any(|t| t.contains("lr")),
            "a leaf must not touch the link register at all: {a:?}"
        );
        assert_eq!(*a.last().expect("nonempty"), "ret");
        assert!(
            after.fns["blend"].code.len() < before.fns["blend"].code.len(),
            "and the two words must be gone: {} -> {}",
            before.fns["blend"].code.len(),
            after.fns["blend"].code.len()
        );
    }

    #[test]
    fn a_function_whose_values_all_fit_gets_no_frame() {
        const SRC: &str = r#"
module examples.item_f_no_frame

pub fn nothing():
    pass
"#;
        let before = emit(SRC, E);
        let after = emit(SRC, &release_without_item_j());
        assert!(
            frame_of(&before, "nothing") > 0,
            "item E framed even this; if not, the oracle is vacuous"
        );
        assert_eq!(frame_of(&after, "nothing"), 0, "item F3 must delete it");
        let a = mnems(&after, "nothing");
        assert!(
            !a.iter()
                .any(|t| t.starts_with("sub sp") || t.starts_with("add sp")),
            "a frameless fn must not adjust sp: {a:?}"
        );
        assert_eq!(rule_count(&after, "nothing", CostRule::Load), 0);
        assert_eq!(rule_count(&after, "nothing", CostRule::Store), 0);
    }

    const TAIL: &str = r#"
module examples.item_f_tail

fn add_one(x: u64) -> u64:
    return x +% 1

pub fn use_it(x: u64) -> u64:
    return add_one(x)
"#;

    #[test]
    fn a_tail_call_emits_b_not_bl_and_ret() {
        let before = emit(TAIL, E);
        let after = emit(TAIL, &release_without_item_j());

        let b = mnems(&before, "use_it");
        assert!(
            b.iter().any(|t| *t == "bl <add_one>"),
            "before item F this must be a linking call: {b:?}"
        );

        let a = mnems(&after, "use_it");
        assert!(
            a.iter().any(|t| t.starts_with("b <add_one>")),
            "item F5 must emit a jump: {a:?}"
        );
        assert!(
            !a.iter().any(|t| *t == "bl <add_one>"),
            "no linking call may survive: {a:?}"
        );
        assert_eq!(
            rule_count(&after, "use_it", CostRule::Call),
            0,
            "a tail call is a branch, not a call"
        );
        assert!(
            after.fns["use_it"]
                .relocs
                .iter()
                .any(|r| matches!(r, Reloc::Call { key, .. } if key == "add_one")),
            "the call edge must survive for layout and reachability"
        );
        assert!(
            after.fns["use_it"].code.len() < before.fns["use_it"].code.len(),
            "the tail call must not cost words: {} -> {}",
            before.fns["use_it"].code.len(),
            after.fns["use_it"].code.len()
        );
    }

    #[test]
    fn a_non_tail_call_is_still_a_linking_call() {
        const SRC: &str = r#"
module examples.item_f_not_tail

fn add_one(x: u64) -> u64:
    return x +% 1

pub fn twice(x: u64) -> u64:
    y: u64 = add_one(x)
    return y +% y
"#;
        let prog = emit(SRC, &release_without_item_j());
        let a = mnems(&prog, "twice");
        assert!(
            a.iter().any(|t| *t == "bl <add_one>"),
            "a non-tail call must still link: {a:?}"
        );
    }

    #[test]
    fn a_value_survives_a_call_in_a_register_the_callee_does_not_clobber() {
        const SRC: &str = r#"
module examples.item_f_across_call

fn small(a: u64) -> u64:
    return a +% 1

pub fn spans(a: u64) -> u64:
    keep: u64 = a *% 3
    p: u64 = small(a)
    q: u64 = small(p)
    return (keep +% p) +% (q +% keep)
"#;
        let before = emit(SRC, E);
        let after = emit(SRC, &release_without_item_j());
        let bl = rule_count(&before, "spans", CostRule::Load);
        let al = rule_count(&after, "spans", CostRule::Load);
        assert!(
            al < bl,
            "cross-call residency must delete reloads: {bl} -> {al}"
        );
        assert!(
            frame_of(&after, "spans") < frame_of(&before, "spans"),
            "and the frame must shrink with them: {} -> {}",
            frame_of(&before, "spans"),
            frame_of(&after, "spans")
        );
        let conv = after
            .conventions
            .get("spans")
            .expect("every sync fn gets a convention under release");
        assert!(
            conv.assignment.resident_count() > 0,
            "the caller must have residents to have kept anything"
        );
        let small = after.conventions.get("small").expect("callee convention");
        assert!(
            !small.opaque,
            "a leaf's clobber set must be measured, not the fail-closed answer"
        );
        assert_ne!(small.clobbers, regalloc::ALL_REGS);
    }

    #[test]
    fn an_unheld_callee_clobbers_everything() {
        use std::collections::BTreeMap;
        let facts = regalloc::FnFacts {
            temp_count: 1,
            points: vec![
                regalloc::PointFacts::default(),
                regalloc::PointFacts::default(),
            ],
            back_edges: Vec::new(),
            calls: ["nowhere".to_string()].into_iter().collect(),
            opaque_calls: false,
            has_returning_call: true,
        };
        let mut fns = BTreeMap::new();
        fns.insert(
            "f".to_string(),
            regalloc::FnInput {
                facts,
                scalar_slot: vec![true],
                opaque_body: false,
            },
        );
        let out = regalloc::allocate_program(&fns);
        assert_eq!(out["f"].clobbers, regalloc::ALL_REGS);
        assert!(out["f"].opaque);
    }

    #[test]
    fn the_pool_reaches_past_item_es_nine_registers() {
        let after = emit(LEAF, &release_without_item_j());
        let conv = after.conventions.get("blend").expect("convention");
        assert!(
            conv.pool.len() > regalloc::POOL.len(),
            "item F's pool must be wider than item E's nine: {} vs {}",
            conv.pool.len(),
            regalloc::POOL.len()
        );
        assert!(
            conv.pool.iter().any(|r| !regalloc::POOL.contains(r)),
            "and must actually contain a register item E could not reach"
        );
        for r in &conv.pool {
            assert!(
                ![18u8, 28, 29, 30, 31].contains(r),
                "x{r} is reserved and must never enter a pool"
            );
        }
    }

    #[test]
    fn every_key_a_later_stage_may_own_is_opaque_to_the_allocator() {
        let owned_by_layout: Vec<String> = vec![
            "__wrela_abort_tail".to_string(),
            "__test_call_0".to_string(),
            "__test_prefix_0".to_string(),
            "__method_0".to_string(),
            "__enqueue_0".to_string(),
            "__resume_0".to_string(),
            "__boot_call_0".to_string(),
            "__irq_call_0".to_string(),
            rt_boot_init_symbol(),
            rt_enqueue_symbol("Actor"),
            rt_secondary_core_entry_symbol(1),
        ];
        for key in &owned_by_layout {
            assert!(
                is_compiler_glue_symbol(key) || key.starts_with("__"),
                "`{key}` is a key layout may replace, but the allocator would \
                 give it a measured clobber set and every caller of it would \
                 be compiled against a body that is not the one that runs"
            );
        }
        for key in [
            "chain",
            "Outer.relay",
            "copy_bytes_range",
            "fn:identity[u64]",
        ] {
            assert!(
                !(is_compiler_glue_symbol(key) || key.starts_with("__")),
                "`{key}` is an ordinary source fn and must keep a measured \
                 clobber set"
            );
        }
    }

    #[test]
    fn verify_conventions_refuses_a_clobber_set_the_code_exceeds() {
        let mut program = CodegenProgram::default();
        let mut f = CodegenFn {
            frame_size: 0,
            code: Vec::new(),
            relocs: Vec::new(),
        };
        f.code.push(EmittedWord::new(
            encode::enc_mov_reg(4, 9, true),
            "mov x4, x9".to_string(),
            CostRule::Alu,
            Some(4),
            &[9],
        ));
        program.fns.insert("victim".to_string(), f);
        program
            .conventions
            .insert("victim".to_string(), regalloc::Convention::default());
        let err = verify_conventions(&program).expect_err("understated clobbers must be refused");
        assert!(err.contains("victim"), "{err}");
        assert!(err.contains("x4") && err.contains("x9"), "{err}");

        let mut ok = program.clone();
        ok.conventions.get_mut("victim").expect("present").clobbers =
            regalloc::reg_bit(4) | regalloc::reg_bit(9);
        verify_conventions(&ok).expect("an honest clobber set must pass");
    }

    #[test]
    fn verify_conventions_refuses_a_clobber_set_a_callee_exceeds() {
        let mut program = CodegenProgram::default();
        let mut caller = CodegenFn {
            frame_size: 0,
            code: Vec::new(),
            relocs: Vec::new(),
        };
        caller.code.push(EmittedWord::new(
            encode::enc_bl(0),
            "bl <callee>".to_string(),
            CostRule::Call,
            Some(0),
            &[],
        ));
        caller.relocs.push(Reloc::Call {
            word: 0,
            key: "callee".to_string(),
        });
        program.fns.insert("caller".to_string(), caller);
        program.fns.insert(
            "callee".to_string(),
            CodegenFn {
                frame_size: 0,
                code: Vec::new(),
                relocs: Vec::new(),
            },
        );
        program.conventions.insert(
            "caller".to_string(),
            regalloc::Convention {
                clobbers: regalloc::reg_bit(0),
                ..Default::default()
            },
        );
        program.conventions.insert(
            "callee".to_string(),
            regalloc::Convention {
                clobbers: regalloc::reg_bit(0) | regalloc::reg_bit(19),
                ..Default::default()
            },
        );

        let err = verify_conventions(&program).expect_err("a callee's clobbers must propagate");
        assert!(err.contains("caller") && err.contains("callee"), "{err}");
        assert!(err.contains("x19"), "{err}");
    }

    #[test]
    fn an_unconventioned_callee_forces_its_caller_to_be_opaque() {
        let mut program = CodegenProgram::default();
        let mut caller = CodegenFn {
            frame_size: 0,
            code: Vec::new(),
            relocs: Vec::new(),
        };
        caller.code.push(EmittedWord::new(
            encode::enc_bl(0),
            "bl <glue>".to_string(),
            CostRule::Call,
            Some(0),
            &[],
        ));
        caller.relocs.push(Reloc::Call {
            word: 0,
            key: "glue".to_string(),
        });
        program.fns.insert("caller".to_string(), caller);
        program.conventions.insert(
            "caller".to_string(),
            regalloc::Convention {
                clobbers: regalloc::reg_bit(0),
                ..Default::default()
            },
        );
        assert!(
            verify_conventions(&program).is_err(),
            "a call into a body this compiler does not hold must force ALL_REGS"
        );

        program
            .conventions
            .get_mut("caller")
            .expect("present")
            .clobbers = regalloc::ALL_REGS;
        verify_conventions(&program).expect("ALL_REGS covers anything");
    }

    #[test]
    fn the_whole_program_convention_is_deterministic() {
        let a = emit(LEAF, RELEASE_OPTS);
        for _ in 0..4 {
            let b = emit(LEAF, RELEASE_OPTS);
            assert_eq!(b.conventions, a.conventions);
        }
    }
}

#[cfg(test)]
mod item_i_tests {
    use super::regalloc_tests::emit;
    use super::*;
    use crate::opts::{OptId, RELEASE_OPTS};

    fn movs(prog: &CodegenProgram, key: &str) -> Vec<(u8, u8)> {
        prog.fns
            .get(key)
            .unwrap_or_else(|| panic!("no fn `{key}` in {:?}", prog.fns.keys().collect::<Vec<_>>()))
            .code
            .iter()
            .filter(|w| w.text.starts_with("mov x") && w.text.contains(", x"))
            .filter_map(|w| Some((w.dst?, *w.srcs[..w.src_len as usize].first()?)))
            .collect()
    }

    fn words(prog: &CodegenProgram, key: &str) -> usize {
        prog.fns.get(key).expect("fn present").code.len()
    }

    fn named_regs(prog: &CodegenProgram, key: &str) -> BTreeSet<u8> {
        let mut out = BTreeSet::new();
        for w in &prog.fns[key].code {
            if let Some(d) = w.dst {
                out.insert(d);
            }
            for &s in &w.srcs[..w.src_len as usize] {
                out.insert(s);
            }
        }
        out
    }

    const CHAIN: &str = r#"
module examples.coalesce_chain

pub fn chain(a: u64) -> u64:
    x: u64 = a +% 1
    y: u64 = x
    z: u64 = y
    return z +% z
"#;

    #[test]
    fn a_copy_between_non_interfering_values_emits_nothing() {
        let after = emit(CHAIN, RELEASE_OPTS);
        assert!(
            movs(&after, "chain").is_empty(),
            "the copy chain still moves registers: {:?} in\n{}",
            movs(&after, "chain"),
            after.fns["chain"]
                .code
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn coalescing_makes_the_allocated_form_smaller_than_the_spilled_one() {
        let without: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::RegAlloc && *o != OptId::InterprocRegs)
            .collect();
        let base = emit(CHAIN, &without);
        let after = emit(CHAIN, RELEASE_OPTS);
        assert!(
            words(&after, "chain") < words(&base, "chain"),
            "the allocated form must be smaller, not merely different: {} -> {}",
            words(&base, "chain"),
            words(&after, "chain")
        );
    }

    const OVERLAP: &str = r#"
module examples.coalesce_overlap

pub fn overlap(a: u64, b: u64) -> u64:
    p: u64 = a +% 1
    q: u64 = b +% 2
    r: u64 = p +% q
    return r +% p
"#;

    #[test]
    fn two_interfering_values_are_not_coalesced() {
        let after = emit(OVERLAP, RELEASE_OPTS);
        let mut checked = 0usize;
        for w in &after.fns["overlap"].code {
            let Some(rest) = w.text.strip_prefix("add ") else {
                continue;
            };
            let ops: Vec<&str> = rest.split(", ").collect();
            if ops.len() != 3 || !ops[2].starts_with('x') {
                continue;
            }
            checked += 1;
            assert_ne!(
                ops[1], ops[2],
                "`{}` reads one register twice: two simultaneously live values \
                 were coalesced onto one home",
                w.text
            );
        }
        assert!(checked >= 2, "the program must still add ({checked} found)");
    }

    const CALLARG: &str = r#"
module examples.coalesce_callarg

fn sink(v: u64) -> u64:
    return v +% v

pub fn forward(a: u64) -> u64:
    n: u64 = a +% 7
    return sink(n)
"#;

    #[test]
    fn residency_no_longer_costs_more_words_than_it_saves() {
        let without: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::RegAlloc && *o != OptId::InterprocRegs)
            .collect();
        for src in [CHAIN, OVERLAP, CALLARG] {
            let base = emit(src, &without);
            let after = emit(src, RELEASE_OPTS);
            for key in base.fns.keys() {
                let (b, a) = (words(&base, key), words(&after, key));
                assert!(
                    a <= b,
                    "fn `{key}`: the allocator grew the code, {b} -> {a} words"
                );
            }
        }
    }
}

#[cfg(test)]
mod b4_tests {
    use super::regalloc_tests::emit;
    use super::*;
    use crate::opts::{CompileMode, OptId, RELEASE_OPTS, apply_mode, apply_opts};

    #[test]
    fn b4_refuses_every_elision_that_would_merge_a_lane_2_block() {
        let body = vec![
            Inst::Jump { target: 1 },
            Inst::Jump { target: 3 },
            Inst::Return { value: None },
            Inst::Return { value: None },
        ];
        let leaders = mwir_block_leaders(&body);
        assert_eq!(
            leaders,
            vec![true, true, true, true],
            "every branch makes its successor a leader — that is why the \
             refusal admits only the final index"
        );

        apply_opts(&[]);
        assert_eq!(
            sync_branch_elision(&body),
            vec![false; 4],
            "the opt is off: the plan must be empty"
        );

        apply_opts(&[OptId::BranchCleanup]);
        assert_eq!(
            sync_branch_elision(&body),
            vec![false, false, false, true],
            "only the final branch may go"
        );
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn b4_elides_nothing_when_the_body_does_not_end_in_a_branch() {
        apply_opts(&[OptId::BranchCleanup]);
        let body = vec![
            Inst::Return { value: None },
            Inst::AssertFail { message: None },
        ];
        assert_eq!(sync_branch_elision(&body), vec![false, false]);
        apply_mode(CompileMode::Release);
    }

    const TWO_FNS: &str = r#"
module examples.b4_trailing_branch

pub fn leaf(x: u64) -> u64:
    return x +% 1

pub fn two(x: u64) -> u64:
    if x > 3:
        return x +% 1
    return x +% 2
"#;

    #[test]
    fn b4_deletes_the_trailing_branch_word_and_moves_nothing_else() {
        let without: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::BranchCleanup)
            .collect();
        let off = emit(TWO_FNS, &without);
        let on = emit(TWO_FNS, RELEASE_OPTS);
        apply_mode(CompileMode::Release);

        assert_eq!(
            off.fns.keys().collect::<Vec<_>>(),
            on.fns.keys().collect::<Vec<_>>()
        );
        for (key, f_off) in &off.fns {
            let f_on = &on.fns[key];
            assert_eq!(
                f_on.code.len() + 1,
                f_off.code.len(),
                "fn `{key}` must be exactly one word shorter under B4"
            );
            fn mn(f: &CodegenFn) -> Vec<&str> {
                f.code
                    .iter()
                    .map(|w| w.text.split(' ').next().unwrap_or(""))
                    .collect()
            }
            let (a, b) = (mn(f_off), mn(f_on));
            let at = (0..b.len()).find(|&i| a[i] != b[i]).unwrap_or(b.len());
            assert_eq!(a[at], "b", "fn `{key}`: B4 deleted something else");
            assert_eq!(
                f_off.code[at].text, "b #4",
                "fn `{key}`: the deleted branch must target the very next \
                 word (one word = 4 bytes)"
            );
            assert_eq!(
                &a[at + 1..],
                &b[at..],
                "fn `{key}`: every instruction after the deleted branch must \
                 still be there, in order — a two-pass disagreement about the \
                 plan would have desynchronized the stream"
            );
            assert_eq!(
                &a[..at],
                &b[..at],
                "fn `{key}`: instructions before it moved"
            );
        }
    }
}
