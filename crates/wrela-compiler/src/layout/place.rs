use std::collections::BTreeMap;

use super::{
    ActorAddrs, MAILBOX_BOOKKEEPING_SIZE, RR_CURSOR_SIZE, RingAddrs, RuntimePlacement,
    RuntimeTables, TurnId, ring_data_stride_bytes,
};

pub fn place_runtime_tables(base: u64, tables: &RuntimeTables) -> RuntimePlacement {
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
            turn: turn_addr(i),
        });
    }
    let mut drivers = Vec::with_capacity(tables.drivers.len());
    let mut driver_mailboxes = BTreeMap::new();
    let mut next_turn = tables.actors.len();
    for (i, d) in tables.drivers.iter().enumerate() {
        let state = cursor;
        drivers.push(state);
        cursor += d.state_size;
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
    let n_rings = tables.rings.len() as u64;
    let stride = if tables.rings.is_empty() {
        0
    } else {
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
