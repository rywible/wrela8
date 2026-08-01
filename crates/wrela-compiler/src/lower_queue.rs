use crate::mwir::Inst;
use crate::mwir::Temp;
use crate::sema::types::Type;
use crate::syntax::ast::BinOp;
use crate::virtqueue;

pub fn virtqueue_depth_of(ty: &Type) -> Result<u16, String> {
    let Type::Named(name, targs) = ty else {
        return Err(format!(
            "queue temp is `{}`, not `VirtQueue[..N]`",
            crate::sema::types::render_type(ty)
        ));
    };
    if name != "VirtQueue" {
        return Err(format!("queue temp is `{name}`, not `VirtQueue[..N]`"));
    }
    let Some(crate::sema::types::TypeArg::Bound(expr)) = targs.first() else {
        return Err("`VirtQueue` with no bound depth".to_string());
    };
    let text = match expr {
        crate::syntax::ast::Expr::Int(_, t) => t.as_str(),
        _ => {
            return Err("`VirtQueue[..N]` whose depth is not an integer literal".to_string());
        }
    };
    let n: u64 = text
        .parse()
        .map_err(|_| format!("`VirtQueue[..{text}]` depth is not a u64 literal"))?;
    u16::try_from(n).map_err(|_| format!("`VirtQueue[..{n}]` depth does not fit u16"))
}

fn place(depth: u16) -> Result<virtqueue::RingPlacement, String> {
    virtqueue::place_ring(0, depth)
        .ok_or_else(|| format!("place_ring(0, {depth}) refused a proven depth"))
}

pub trait QueueSink {
    fn fresh(&mut self, ty: Type) -> Temp;
    fn emit(&mut self, inst: Inst) -> usize;
    fn here(&mut self) -> usize;
    fn patch(&mut self, idx: usize, target: usize);
}

fn imm(e: &mut dyn QueueSink, v: i128) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::ConstInt {
        dst: t,
        ty: Type::U64,
        value: v,
    });
    t
}

fn ptr_off(e: &mut dyn QueueSink, base: Temp, offset: u64) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::PtrOffset {
        dst: t,
        base,
        offset,
    });
    t
}

fn load(e: &mut dyn QueueSink, base: Temp, offset: u64, width: u8) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::MemLoad {
        dst: t,
        base,
        offset,
        width,
    });
    t
}

fn store(e: &mut dyn QueueSink, base: Temp, offset: u64, value: Temp, width: u8) {
    e.emit(Inst::MemStore {
        base,
        offset,
        value,
        width,
    });
}

fn wrap_add(e: &mut dyn QueueSink, lhs: Temp, rhs: Temp) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::ArithWrapping {
        dst: t,
        op: BinOp::AddW,
        ty: Type::U64,
        lhs,
        rhs,
    });
    t
}

fn wrap_sub(e: &mut dyn QueueSink, lhs: Temp, rhs: Temp) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::ArithWrapping {
        dst: t,
        op: BinOp::SubW,
        ty: Type::U64,
        lhs,
        rhs,
    });
    t
}

fn bit_and(e: &mut dyn QueueSink, lhs: Temp, rhs: Temp) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::Bitwise {
        dst: t,
        op: BinOp::BitAnd,
        ty: Type::U64,
        lhs,
        rhs,
    });
    t
}

fn bit_or(e: &mut dyn QueueSink, lhs: Temp, rhs: Temp) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::Bitwise {
        dst: t,
        op: BinOp::BitOr,
        ty: Type::U64,
        lhs,
        rhs,
    });
    t
}

fn bit_not(e: &mut dyn QueueSink, src: Temp) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::BitNot {
        dst: t,
        ty: Type::U64,
        src,
    });
    t
}

fn shl1(e: &mut dyn QueueSink, src: Temp) -> Temp {
    wrap_add(e, src, src)
}

fn shl3(e: &mut dyn QueueSink, src: Temp) -> Temp {
    let eight = imm(e, 8);
    let t = e.fresh(Type::U64);
    e.emit(Inst::ArithWrapping {
        dst: t,
        op: BinOp::MulW,
        ty: Type::U64,
        lhs: src,
        rhs: eight,
    });
    t
}

fn cmp(e: &mut dyn QueueSink, op: BinOp, lhs: Temp, rhs: Temp) -> Temp {
    let t = e.fresh(Type::Bool);
    e.emit(Inst::Compare {
        dst: t,
        op,
        ty: Type::U64,
        lhs,
        rhs,
    });
    t
}

fn project(e: &mut dyn QueueSink, base: Temp, index: usize) -> Temp {
    let t = e.fresh(Type::U64);
    e.emit(Inst::Project {
        dst: t,
        base,
        index,
    });
    t
}

fn abort_msg(e: &mut dyn QueueSink, message: &str) {
    e.emit(Inst::Abort {
        message: message.to_string(),
    });
}

fn jump(e: &mut dyn QueueSink) -> usize {
    e.emit(Inst::Jump { target: usize::MAX })
}

fn jump_if(e: &mut dyn QueueSink, cond: Temp) -> usize {
    let fall = e.emit(Inst::JumpIfFalse {
        cond,
        target: usize::MAX,
    });
    let j = e.emit(Inst::Jump { target: usize::MAX });
    let fall_pos = e.here();
    e.patch(fall, fall_pos);
    j
}

fn abort_unless(e: &mut dyn QueueSink, cond: Temp, message: &str) {
    let to_ok = jump_if(e, cond);
    abort_msg(e, message);
    let after = e.here();
    e.patch(to_ok, after);
}

fn desc_entry(
    e: &mut dyn QueueSink,
    pool: Temp,
    desc_base: u64,
    desc_index: u16,
    addr: Temp,
    len: Temp,
    flags: Temp,
    next: u16,
) {
    let entry = ptr_off(e, pool, desc_base + desc_index as u64 * 16);
    store(e, entry, 0, addr, 8);
    store(e, entry, 8, len, 4);
    store(e, entry, 12, flags, 2);
    let n = imm(e, next as i128);
    store(e, entry, 14, n, 2);
}

pub fn expand_suppress(queue: Temp, depth: u16, e: &mut dyn QueueSink) -> Result<(), String> {
    let placed = place(depth)?;
    let avail = ptr_off(e, queue, placed.avail);
    let flags = imm(e, virtqueue::AVAIL_F_NO_INTERRUPT as i128);
    store(e, avail, 0, flags, 2);
    Ok(())
}

pub fn expand_prepare(
    dst: Temp,
    queue: Temp,
    header: Temp,
    payload: Temp,
    status: Temp,
    device_writes: bool,
    payload_len: u32,
    depth: u16,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let placed = place(depth)?;
    let meta = virtqueue::meta_offset(placed.bytes);
    let header_off = meta + virtqueue::SLOT_META_BYTES;
    let status_off = header_off + virtqueue::REQ_HEADER_SIZE;

    let hdr_dst = ptr_off(e, queue, header_off);
    let kind = project(e, header, 0);
    store(e, hdr_dst, 0, kind, 4);
    let reserved = project(e, header, 1);
    store(e, hdr_dst, 4, reserved, 4);
    let sector = project(e, header, 2);
    store(e, hdr_dst, 8, sector, 8);

    let st_dst = ptr_off(e, queue, status_off);
    store(e, st_dst, 0, status, 1);

    let meta_ptr = ptr_off(e, queue, meta);
    store(e, meta_ptr, virtqueue::SLOT_META_PAYLOAD, payload, 8);
    let hdr_addr = ptr_off(e, queue, header_off);
    store(e, meta_ptr, virtqueue::SLOT_META_HEADER, hdr_addr, 8);
    let st_addr = ptr_off(e, queue, status_off);
    store(e, meta_ptr, virtqueue::SLOT_META_STATUS, st_addr, 8);
    let plen = imm(e, payload_len as i128);
    store(e, meta_ptr, virtqueue::SLOT_META_PAYLOAD_LEN, plen, 8);
    let flags = virtqueue::SLOT_FLAG_INFLIGHT
        | if device_writes {
            virtqueue::SLOT_FLAG_DEVICE_WRITES
        } else {
            0
        };
    let flags_t = imm(e, flags as i128);
    store(e, meta_ptr, virtqueue::SLOT_META_FLAGS, flags_t, 8);

    let epoch_addr = ptr_off(e, queue, placed.bytes + virtqueue::SLOT_BOOK_EPOCH);
    let epoch = load(e, epoch_addr, 0, 8);
    store(e, meta_ptr, virtqueue::SLOT_META_EPOCH, epoch, 8);

    let zero = imm(e, 0);
    store(e, meta_ptr, virtqueue::SLOT_META_WAITER, zero, 4);
    store(e, meta_ptr, virtqueue::SLOT_META_REPLY_STAGE, zero, 8);

    e.emit(Inst::Copy { dst, src: meta_ptr });
    Ok(())
}

pub fn expand_publish(
    dst: Temp,
    queue: Temp,
    operation: Temp,
    depth: u16,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let _ = virtqueue::PUBLISH_WRITE_ORDER;
    let placed = place(depth)?;

    let header_addr = load(e, operation, virtqueue::SLOT_META_HEADER, 8);
    let hlen = imm(e, virtqueue::REQ_HEADER_SIZE as i128);
    let hflags = imm(e, virtqueue::DESC_F_NEXT as i128);
    desc_entry(e, queue, placed.desc, 0, header_addr, hlen, hflags, 1);

    let payload_addr = load(e, operation, virtqueue::SLOT_META_PAYLOAD, 8);
    let plen = load(e, operation, virtqueue::SLOT_META_PAYLOAD_LEN, 8);
    let meta_flags = load(e, operation, virtqueue::SLOT_META_FLAGS, 8);
    let dw = imm(e, virtqueue::SLOT_FLAG_DEVICE_WRITES as i128);
    let dw_bit = bit_and(e, meta_flags, dw);
    let dw_shifted = shl1(e, dw_bit);
    let next_flag = imm(e, virtqueue::DESC_F_NEXT as i128);
    let data_flags = bit_or(e, dw_shifted, next_flag);
    desc_entry(e, queue, placed.desc, 1, payload_addr, plen, data_flags, 2);

    let status_addr = load(e, operation, virtqueue::SLOT_META_STATUS, 8);
    let slen = imm(e, virtqueue::REQ_STATUS_SIZE as i128);
    let sflags = imm(e, virtqueue::DESC_F_WRITE as i128);
    desc_entry(e, queue, placed.desc, 2, status_addr, slen, sflags, 0);

    let avail = ptr_off(e, queue, placed.avail);
    let idx = load(e, avail, 2, 2);
    let mask = imm(e, (depth as u64 - 1) as i128);
    let slot = bit_and(e, idx, mask);
    let slot2 = shl1(e, slot);
    let four = imm(e, 4);
    let ring_off = wrap_add(e, slot2, four);
    let ring_ptr = wrap_add(e, avail, ring_off);
    let zero = imm(e, 0);
    store(e, ring_ptr, 0, zero, 2);
    let one = imm(e, 1);
    let idx1 = wrap_add(e, idx, one);
    store(e, avail, 2, idx1, 2);

    let doorbell = ptr_off(e, queue, placed.doorbell);
    store(e, doorbell, 0, one, 8);

    e.emit(Inst::Copy {
        dst,
        src: operation,
    });
    Ok(())
}

fn doorbell_poll_park(e: &mut dyn QueueSink) {
    let clock = imm(e, wrela_machine::mmio::CLOCK_MMIO_ADDR as i128);
    let now = load(e, clock, 0, 8);
    let delay = imm(e, 20_000_000);
    let deadline = wrap_add(e, now, delay);
    let deadline_addr = imm(
        e,
        (wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_NEXT_DEADLINE)
            as i128,
    );
    store(e, deadline_addr, 0, deadline, 8);
    let park = imm(e, wrela_machine::mmio::PARK_MMIO_ADDR as i128);
    store(e, park, 0, deadline, 8);
}

pub fn expand_drain(
    queue: Temp,
    _max: u16,
    depth: u16,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let placed = place(depth)?;
    let meta_off = virtqueue::meta_offset(placed.bytes);
    let comp_off = virtqueue::completion_offset(placed.bytes);
    let book_off = placed.bytes;

    let book = ptr_off(e, queue, book_off);
    let last = load(e, book, 0, 8);
    let used = ptr_off(e, queue, placed.used);
    let used_idx = load(e, used, 2, 2);
    let pending = wrap_sub(e, used_idx, last);
    let zero = imm(e, 0);
    let pending_nz = cmp(e, BinOp::Ne, pending, zero);
    let skip_empty = jump_if(e, pending_nz);

    doorbell_poll_park(e);
    let book2 = ptr_off(e, queue, book_off);
    let last2 = load(e, book2, 0, 8);
    let used2 = ptr_off(e, queue, placed.used);
    let used_idx2 = load(e, used2, 2, 2);
    let pending2 = wrap_sub(e, used_idx2, last2);
    let pending2_nz = cmp(e, BinOp::Ne, pending2, zero);
    let skip_still = jump_if(e, pending2_nz);
    let done_empty = jump(e);
    let still_pos = e.here();
    e.patch(skip_still, still_pos);
    let empty_join = e.here();
    e.patch(skip_empty, empty_join);

    let book3 = ptr_off(e, queue, book_off);
    let last3 = load(e, book3, 0, 8);
    let used3 = ptr_off(e, queue, placed.used);
    let mask = imm(e, (depth as u64 - 1) as i128);
    let slot = bit_and(e, last3, mask);
    let slot8 = shl3(e, slot);
    let four = imm(e, 4);
    let entry_off = wrap_add(e, slot8, four);
    let entry = wrap_add(e, used3, entry_off);
    let id = load(e, entry, 0, 4);
    let used_len = load(e, entry, 4, 4);

    let expected = imm(e, virtqueue::EXPECTED_HEAD as i128);
    let id_ok = cmp(e, BinOp::Eq, id, expected);
    abort_unless(
        e,
        id_ok,
        &virtqueue::CompletionFault::UnknownId { id: 0 }.abort_message(),
    );

    let meta = ptr_off(e, queue, meta_off);
    let epoch_addr = ptr_off(e, queue, book_off + virtqueue::SLOT_BOOK_EPOCH);
    let cur_epoch = load(e, epoch_addr, 0, 8);
    let slot_epoch = load(e, meta, virtqueue::SLOT_META_EPOCH, 8);
    let epoch_ok = cmp(e, BinOp::Eq, slot_epoch, cur_epoch);
    abort_unless(
        e,
        epoch_ok,
        &virtqueue::CompletionFault::StaleId {
            id: 0,
            slot_epoch: 0,
            current_epoch: 0,
        }
        .abort_message(),
    );

    let flags = load(e, meta, virtqueue::SLOT_META_FLAGS, 8);
    let inflight_m = imm(e, virtqueue::SLOT_FLAG_INFLIGHT as i128);
    let inflight = bit_and(e, flags, inflight_m);
    let inflight_ok = cmp(e, BinOp::Ne, inflight, zero);
    abort_unless(
        e,
        inflight_ok,
        &virtqueue::CompletionFault::DuplicateId { id: 0 }.abort_message(),
    );

    let payload_len = load(e, meta, virtqueue::SLOT_META_PAYLOAD_LEN, 8);
    let dw_m = imm(e, virtqueue::SLOT_FLAG_DEVICE_WRITES as i128);
    let dw = bit_and(e, flags, dw_m);
    let one = imm(e, 1);
    let len_ge1 = cmp(e, BinOp::Ge, used_len, one);
    abort_unless(
        e,
        len_ge1,
        &virtqueue::CompletionFault::BadLength {
            reported: 0,
            capacity: 0,
        }
        .abort_message(),
    );
    let buffer_facing = wrap_sub(e, used_len, one);
    let is_write = cmp(e, BinOp::Ne, dw, zero);
    let to_in = jump_if(e, is_write);
    let out_ok = cmp(e, BinOp::Eq, buffer_facing, zero);
    abort_unless(
        e,
        out_ok,
        &virtqueue::CompletionFault::BadLength {
            reported: 0,
            capacity: 0,
        }
        .abort_message(),
    );
    let after_len = jump(e);
    let in_pos = e.here();
    e.patch(to_in, in_pos);
    let in_ok = cmp(e, BinOp::Le, buffer_facing, payload_len);
    abort_unless(
        e,
        in_ok,
        &virtqueue::CompletionFault::BadLength {
            reported: 0,
            capacity: 0,
        }
        .abort_message(),
    );
    let after_len_pos = e.here();
    e.patch(after_len, after_len_pos);

    let status_ptr = load(e, meta, virtqueue::SLOT_META_STATUS, 8);
    let status_b = load(e, status_ptr, 0, 1);
    let payload = load(e, meta, virtqueue::SLOT_META_PAYLOAD, 8);
    let comp = ptr_off(e, queue, comp_off);
    store(e, comp, 0, payload, 8);
    let status_ok = cmp(e, BinOp::Eq, status_b, zero);
    let tag_ok = imm(e, 0);
    let tag_err = imm(e, 1);
    let tag_sel_j = jump_if(e, status_ok);
    store(e, comp, 8, tag_err, 8);
    let tag_done = jump(e);
    let tag_ok_pos = e.here();
    e.patch(tag_sel_j, tag_ok_pos);
    store(e, comp, 8, tag_ok, 8);
    let tag_done_pos = e.here();
    e.patch(tag_done, tag_done_pos);
    store(e, comp, 16, zero, 8);
    store(e, comp, 24, buffer_facing, 8);

    let flags2 = load(e, meta, virtqueue::SLOT_META_FLAGS, 8);
    let not_inf = bit_not(e, inflight_m);
    let flags3 = bit_and(e, flags2, not_inf);
    let resolved_m = imm(e, virtqueue::SLOT_FLAG_RESOLVED as i128);
    let flags4 = bit_or(e, flags3, resolved_m);
    store(e, meta, virtqueue::SLOT_META_FLAGS, flags4, 8);

    let stage_turn = load(e, meta, virtqueue::SLOT_META_REPLY_STAGE, 4);
    let stage_nz = cmp(e, BinOp::Ne, stage_turn, zero);
    let do_stage = jump_if(e, stage_nz);
    let no_stage = jump(e);
    let stage_pos = e.here();
    e.patch(do_stage, stage_pos);
    let stage_off = load(e, meta, virtqueue::SLOT_META_REPLY_STAGE + 4, 4);
    let turn_addr = e.fresh(Type::U64);
    e.emit(Inst::TurnAddrFromId {
        dst: turn_addr,
        id: stage_turn,
    });
    let reply = wrap_add(e, turn_addr, stage_off);
    for w in [0u64, 8, 16, 24] {
        let word = load(e, comp, w, 8);
        store(e, reply, w, word, 8);
    }
    let after_stage = e.here();
    e.patch(no_stage, after_stage);

    let waiter = load(e, meta, virtqueue::SLOT_META_WAITER, 4);
    let waiter_nz = cmp(e, BinOp::Ne, waiter, zero);
    let do_waiter = jump_if(e, waiter_nz);
    let no_waiter = jump(e);
    let waiter_pos = e.here();
    e.patch(do_waiter, waiter_pos);
    let waddr = e.fresh(Type::U64);
    e.emit(Inst::TurnAddrFromId {
        dst: waddr,
        id: waiter,
    });
    store(e, waddr, 16, one, 8);
    store(e, meta, virtqueue::SLOT_META_WAITER, zero, 4);
    let after_waiter = e.here();
    e.patch(no_waiter, after_waiter);

    let book4 = ptr_off(e, queue, book_off);
    let last4 = load(e, book4, 0, 8);
    let last5 = wrap_add(e, last4, one);
    store(e, book4, 0, last5, 8);

    let done_pos = e.here();
    e.patch(done_empty, done_pos);
    Ok(())
}

pub fn expand_claim(
    dst: Temp,
    _queue: Temp,
    receipt: Temp,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let flags = load(e, receipt, virtqueue::SLOT_META_FLAGS, 8);
    let resolved_m = imm(e, virtqueue::SLOT_FLAG_RESOLVED as i128);
    let bit = bit_and(e, flags, resolved_m);
    let zero = imm(e, 0);
    let ok = cmp(e, BinOp::Ne, bit, zero);
    abort_unless(
        e,
        ok,
        "driver fault: claim of a receipt that is not RESOLVED",
    );

    let stash_delta = virtqueue::SLOT_META_BYTES + virtqueue::REQ_HEADER_SIZE + 8;
    let stash = ptr_off(e, receipt, stash_delta);
    let w0 = load(e, stash, 0, 8);
    let w1 = load(e, stash, 8, 8);
    let w2 = load(e, stash, 16, 8);
    let w3 = load(e, stash, 24, 8);
    e.emit(Inst::MakeAggregate {
        dst,
        elems: vec![w0, w1, w2, w3],
    });

    let flags2 = load(e, receipt, virtqueue::SLOT_META_FLAGS, 8);
    let not_r = bit_not(e, resolved_m);
    let flags3 = bit_and(e, flags2, not_r);
    store(e, receipt, virtqueue::SLOT_META_FLAGS, flags3, 8);
    Ok(())
}

pub fn expand_recover(
    dst: Temp,
    queue: Temp,
    receipt: Temp,
    depth: u16,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let placed = place(depth)?;
    let epoch_off = placed.bytes + virtqueue::SLOT_BOOK_EPOCH;
    let epoch_addr = ptr_off(e, queue, epoch_off);
    let live = load(e, epoch_addr, 0, 8);
    let stamped = load(e, receipt, virtqueue::SLOT_META_EPOCH, 8);
    let flags = load(e, receipt, virtqueue::SLOT_META_FLAGS, 8);
    let zero = imm(e, 0);

    let epoch_eq = cmp(e, BinOp::Eq, stamped, live);
    let epoch_live = jump_if(e, epoch_eq);
    let unk = imm(e, virtqueue::OUTCOME_UNKNOWN as i128);
    e.emit(Inst::Copy { dst, src: unk });
    let done_stale = jump(e);
    let epoch_pos = e.here();
    e.patch(epoch_live, epoch_pos);

    let inf_m = imm(e, virtqueue::SLOT_FLAG_INFLIGHT as i128);
    let inf = bit_and(e, flags, inf_m);
    let not_inf = cmp(e, BinOp::Eq, inf, zero);
    let not_inflight = jump_if(e, not_inf);
    e.emit(Inst::Copy { dst, src: unk });
    let done_inf = jump(e);
    let inf_pos = e.here();
    e.patch(not_inflight, inf_pos);

    let res_m = imm(e, virtqueue::SLOT_FLAG_RESOLVED as i128);
    let res = bit_and(e, flags, res_m);
    let res_ok = cmp(e, BinOp::Ne, res, zero);
    abort_unless(
        e,
        res_ok,
        virtqueue::RecoverOutcome::not_recoverable_abort_message(),
    );
    let status_ptr = load(e, receipt, virtqueue::SLOT_META_STATUS, 8);
    let status_b = load(e, status_ptr, 0, 1);
    let status_ok = cmp(e, BinOp::Eq, status_b, zero);
    let completed = imm(e, virtqueue::OUTCOME_COMPLETED as i128);
    let not_completed = imm(e, virtqueue::OUTCOME_NOT_COMPLETED as i128);
    let take_ok = jump_if(e, status_ok);
    e.emit(Inst::Copy {
        dst,
        src: not_completed,
    });
    let done_status = jump(e);
    let ok_pos = e.here();
    e.patch(take_ok, ok_pos);
    e.emit(Inst::Copy {
        dst,
        src: completed,
    });
    let join = e.here();
    e.patch(done_stale, join);
    e.patch(done_inf, join);
    e.patch(done_status, join);

    let flags2 = load(e, receipt, virtqueue::SLOT_META_FLAGS, 8);
    let clear_m = imm(
        e,
        (virtqueue::SLOT_FLAG_INFLIGHT | virtqueue::SLOT_FLAG_RESOLVED) as i128,
    );
    let not_clear = bit_not(e, clear_m);
    let flags3 = bit_and(e, flags2, not_clear);
    let quar_m = imm(e, virtqueue::SLOT_FLAG_QUARANTINED as i128);
    let flags4 = bit_or(e, flags3, quar_m);
    store(e, receipt, virtqueue::SLOT_META_FLAGS, flags4, 8);

    let q_addr = ptr_off(e, queue, placed.bytes + virtqueue::SLOT_BOOK_QUIESCED);
    let q = load(e, q_addr, 0, 8);
    let stamp = ptr_off(
        e,
        queue,
        placed.bytes + virtqueue::SLOT_BOOK_QUARANTINE_STAMP,
    );
    store(e, stamp, 0, q, 8);
    Ok(())
}

pub fn expand_reclaim(
    dst: Temp,
    queue: Temp,
    depth: u16,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let placed = place(depth)?;
    let meta_off = virtqueue::meta_offset(placed.bytes);
    let meta = ptr_off(e, queue, meta_off);
    let flags = load(e, meta, virtqueue::SLOT_META_FLAGS, 8);
    let quar_m = imm(e, virtqueue::SLOT_FLAG_QUARANTINED as i128);
    let bit = bit_and(e, flags, quar_m);
    let zero = imm(e, 0);
    let ok = cmp(e, BinOp::Ne, bit, zero);
    abort_unless(
        e,
        ok,
        &virtqueue::ReclaimGate::NotQuarantined.abort_message(),
    );

    let q_addr = ptr_off(e, queue, placed.bytes + virtqueue::SLOT_BOOK_QUIESCED);
    let q = load(e, q_addr, 0, 8);
    let stamp_addr = ptr_off(
        e,
        queue,
        placed.bytes + virtqueue::SLOT_BOOK_QUARANTINE_STAMP,
    );
    let stamp = load(e, stamp_addr, 0, 8);
    let moved = cmp(e, BinOp::Ne, q, stamp);
    abort_unless(
        e,
        moved,
        &virtqueue::ReclaimGate::NotQuiesced.abort_message(),
    );

    let payload = load(e, meta, virtqueue::SLOT_META_PAYLOAD, 8);
    e.emit(Inst::Copy { dst, src: payload });
    let not_q = bit_not(e, quar_m);
    let flags2 = bit_and(e, flags, not_q);
    store(e, meta, virtqueue::SLOT_META_FLAGS, flags2, 8);
    store(e, meta, virtqueue::SLOT_META_PAYLOAD, zero, 8);
    Ok(())
}

pub fn expand_device_reset(
    dst: Temp,
    device: Temp,
    queue: Temp,
    depth: u16,
    e: &mut dyn QueueSink,
) -> Result<(), String> {
    let placed = place(depth)?;
    let epoch_off = placed.bytes + virtqueue::SLOT_BOOK_EPOCH;
    let quiesced_off = placed.bytes + virtqueue::SLOT_BOOK_QUIESCED;

    let q_count = ptr_off(e, queue, quiesced_off);
    let quiesce_mmio = imm(e, wrela_machine::mmio::QUIESCE_MMIO_ADDR as i128);
    store(e, quiesce_mmio, 0, q_count, 8);

    let epoch_addr = ptr_off(e, queue, epoch_off);
    let epoch = load(e, epoch_addr, 0, 8);
    let max = imm(e, -1);
    let not_max = cmp(e, BinOp::Ne, epoch, max);
    abort_unless(
        e,
        not_max,
        "driver fault: reset epoch exhausted (03-hardware.md §4: identities never wrap)",
    );
    let one = imm(e, 1);
    let next = wrap_add(e, epoch, one);
    store(e, epoch_addr, 0, next, 8);
    e.emit(Inst::Copy { dst, src: device });
    Ok(())
}
