//! `rtdata` placement: `place_runtime_tables`.

use std::collections::BTreeMap;

use super::{
    ActorAddrs, MAILBOX_BOOKKEEPING_SIZE, RR_CURSOR_SIZE, RingAddrs, RuntimePlacement,
    RuntimeTables, TurnId, ring_data_stride_bytes,
};

pub fn place_runtime_tables(base: u64, tables: &RuntimeTables) -> RuntimePlacement {
    // plans/M10.md item 0b (decision 554): the turn array comes first and
    // whole, so `turns_base == base` and element `i` sits at a plain
    // stride multiple. Each area is *reserved* at the image-wide uniform
    // stride (item 0a), not at its owner's raw area — the owner's own
    // `frame_size` still says how much of it is live.
    //
    // Deliberately **not** aligned up to the stride: `add`-based indexing
    // does not need it, and `verify_section_sizes`' 8-byte inter-section
    // gap rule reports a larger gap as a producer bug (the prefix every
    // fuzz lane treats as a failure), not as an outcome.
    let turns_base = base;
    let turn_addr = |index: usize| turns_base + (index as u64) * tables.turn_stride;
    let mut cursor = base + tables.n_turns * tables.turn_stride;
    let mut actors = Vec::with_capacity(tables.actors.len());
    for (i, a) in tables.actors.iter().enumerate() {
        let state = cursor;
        cursor += a.state_size;
        let ring = cursor;
        cursor += a.mailbox_capacity * a.slot_size;
        let head = cursor;
        cursor += 8;
        let tail = cursor;
        cursor += 8;
        let count = cursor;
        cursor += 8;
        actors.push(ActorAddrs {
            state,
            ring,
            head,
            tail,
            count,
            // The first `tables.actors.len()` turn-array elements are the
            // actors', in this order.
            turn: turn_addr(i),
        });
    }
    let mut drivers = Vec::with_capacity(tables.drivers.len());
    let mut driver_mailboxes = BTreeMap::new();
    // The turn array continues with the messageable drivers, in
    // `tables.drivers` order (`mailbox_root_names`' own order).
    let mut next_turn = tables.actors.len();
    for (i, d) in tables.drivers.iter().enumerate() {
        let state = cursor;
        drivers.push(state);
        cursor += d.state_size;
        // plans/M8.md item D: same four regions, same order, same
        // arithmetic as the actor loop above.
        if let Some(mb) = &d.mailbox {
            let ring = cursor;
            cursor += mb.capacity * mb.slot_size;
            let head = cursor;
            cursor += 8;
            let tail = cursor;
            cursor += 8;
            let count = cursor;
            cursor += 8;
            let turn = turn_addr(next_turn);
            next_turn += 1;
            driver_mailboxes.insert(
                i,
                ActorAddrs {
                    state,
                    ring,
                    head,
                    tail,
                    count,
                    turn,
                },
            );
        }
    }
    // ...and ends with the free turns, in `tables.free_turns` order.
    let mut free_turns = BTreeMap::new();
    let mut turn_ids = BTreeMap::new();
    for (k, (key, _area)) in tables.free_turns.iter().enumerate() {
        let index = next_turn + k;
        free_turns.insert(key.clone(), turn_addr(index));
        turn_ids.insert(key.clone(), TurnId::from_index(index));
    }
    debug_assert_eq!(
        next_turn + tables.free_turns.len(),
        tables.n_turns as usize,
        "`compute_runtime_tables` and `place_runtime_tables` disagree about how many turns exist"
    );
    // plans/M8.md item C1: one ready-queue table + one round-robin cursor
    // per live core, uniformly strided (each core's pair sits at
    // `base + core * (ready_queue_capacity * 8 + RR_CURSOR_SIZE)`). With
    // `cores == 1` this is byte-for-byte the pre-C1 single reservation.
    let sched_base = cursor;
    let per_core = tables.ready_queue_capacity * 8 + RR_CURSOR_SIZE;
    let mut rr_cursors = Vec::with_capacity(tables.cores);
    for core in 0..tables.cores {
        rr_cursors.push(sched_base + (core as u64) * per_core + tables.ready_queue_capacity * 8);
    }
    cursor = sched_base + (tables.cores as u64) * per_core;
    let group_arena = cursor;
    let group_slot = crate::codegen::group_slot_size(
        tables
            .group_max_children
            .max(crate::codegen::GROUP_MAX_CHILDREN_FLOOR),
    );
    cursor += tables.group_arena_capacity * group_slot;
    // plans/M8.md item C2 / M12 item C (decision 875): cross-core rings
    // last — all CTL records contiguously, then uniformly-strided DATA.
    // Every address above this point is unchanged for an image with none.
    let n_rings = tables.rings.len() as u64;
    let stride = if tables.rings.is_empty() {
        0
    } else {
        // Prefer the value `add_cross_core_rings` recorded; recompute if a
        // unit test built tables by hand without that path.
        let s = tables.ring_stride;
        if s == 0 {
            ring_data_stride_bytes(&tables.rings)
        } else {
            s
        }
    };
    let ctl_base = cursor;
    let data_base = ctl_base + n_rings * MAILBOX_BOOKKEEPING_SIZE;
    let mut rings = Vec::with_capacity(tables.rings.len());
    for (i, _r) in tables.rings.iter().enumerate() {
        let i = i as u64;
        let head = ctl_base + i * MAILBOX_BOOKKEEPING_SIZE;
        rings.push(RingAddrs {
            ring: data_base + i * stride,
            head,
            tail: head + 8,
            count: head + 16,
        });
    }
    let rings_end = data_base + n_rings * stride;
    // M12 item D: contiguous WAKE.wake_pending after rings. Length comes
    // from `wake_pending_addrs` (filled to the drain count before place).
    let n_wake = tables.wake_pending_addrs.len() as u64;
    let wake_base = rings_end;
    let _wake_end = wake_base + n_wake * 8;
    RuntimePlacement {
        turns_base,
        turn_stride: tables.turn_stride,
        turn_ids,
        actors,
        drivers,
        driver_mailboxes,
        free_turns,
        rr_cursors,
        group_arena,
        rings,
        wake_base,
    }
}
