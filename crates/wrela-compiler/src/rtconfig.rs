//! Image runtime config generator (plans/M11.md item D, decisions 700–702 /
//! 709 / 760–769; item E, decisions 780–786; item F, decisions 790–796;
//! item G, decisions 800–809).
//!
//! After `@image` evaluation + placement, the compiler pretty-prints a
//! hidden facts-only module (`core.__image_runtime`) as source text and
//! feeds it through the ordinary front end. No AST synthesis, no loops or
//! decisions in the generated text — consts, `@layout(runtime)` types,
//! `@placed` statics, and exhaustive `match` ladders over comptime indices.

use crate::layout::{RingKind, RuntimeTables, place_runtime_tables};
use crate::syntax::ast::Module;
use crate::syntax::{lexer, parser};
use wrela_machine::layout::RTDATA_BASE;

/// One static `g.start` site (plans/M11.md item F / decision 791).
#[derive(Debug, Clone)]
pub struct ChildSiteFact {
    /// Free-async / group-child callee key (`program.fns` spelling).
    pub callee_key: String,
    /// Slot ordinal within the group (`GROUP_MAX_CHILDREN` bound).
    pub child_index: usize,
    /// Index into `RT.turns` for this child's free-turn area.
    pub turn_index: usize,
}

/// One cross-core ring's placed facts (plans/M11.md item G / decision 800).
#[derive(Debug, Clone)]
pub struct RingFact {
    pub kind: RingKind,
    pub src: usize,
    pub dst: usize,
    pub capacity: u64,
    pub slot_size: u64,
    pub ring_base: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
    /// Request lane: image handle word of the target mailbox root
    /// (decision 801 — handle identity, not type-alone).
    pub target_handle: Option<u64>,
    /// Request lane: mailbox-root name for `rt_enqueue` remap until item J.
    pub target_actor: Option<String>,
}

/// Image-specific facts beyond [`RuntimeTables`] (item F / decision 790;
/// item G / decision 800).
/// Empty vectors yield match ladders that always return 0 (stub / dump).
#[derive(Debug, Clone, Default)]
pub struct RtconfigExtras {
    /// Mailbox-root names on each live core, in `mailbox_root_names` order
    /// filtered by `actor_cores` — the RR select list `rt_run_one` walked.
    pub select_by_core: Vec<Vec<String>>,
    /// `true` when this core has any inbound cross-core ring.
    pub drain_by_core: Vec<bool>,
    /// `g.start` sites in `BTreeMap` key order (same as former inject).
    pub child_sites: Vec<ChildSiteFact>,
    /// Cross-core rings in `RuntimeTables::rings` / placement order.
    pub rings: Vec<RingFact>,
    /// Mailbox-root handle word per root (same order as enqueue stubs).
    pub enqueue_handles: Vec<u64>,
    /// Mailbox-root names parallel to `enqueue_handles` (remap targets).
    pub enqueue_actors: Vec<String>,
}

/// Hidden module address (decision 701). Loader key is `["core", "__image_runtime"]`;
/// the file declares plain `module __image_runtime` like every other `core.*` file.
pub const MODULE_PATH: &[&str] = &["__image_runtime"];

/// Dotted address used in TypedProgram / report maps.
pub const MODULE_ADDR: &str = "core.__image_runtime";

/// Report `Input path=` spelling for the generated module (decision 701 / 764).
pub const GENERATED_INPUT_PATH: &str = "<generated>";

/// Dump stage name (decision 701).
pub const DUMP_STAGE: &str = "rtconfig";

/// Fixed stub pools (decision 791 / 802): `runtime.wr` always imports every
/// stub so spliced match-ladder bodies can Call them. Counts are hard
/// ceilings — generator fails closed if an image needs more.
pub const SELECT_STUB_COUNT: usize = 32;
pub const RESUME_STUB_COUNT: usize = 16;
/// Cross-core ring / xsend / xreply pool (decision 802). Matches the
/// handwritten trampoline pools in `runtime.wr`.
pub const RING_POOL_COUNT: usize = 8;
pub const ENQUEUE_STUB_COUNT: usize = 32;
/// Sentinel edge index from match ladders when no ring matches (decision 801).
pub const NO_EDGE: usize = 255;

/// Batch-1 stub text so `runtime.wr` can import counts / `RT` / `GROUPS`
/// before a real image is evaluated (plans/M11.md item E / decision 780).
/// Addresses are placeholders; live images replace this via [`generate`].
pub fn stub_text() -> String {
    let mut tables = RuntimeTables {
        n_turns: 0,
        turn_stride: 0,
        ready_queue_capacity: 1,
        group_arena_capacity: 0,
        total_bytes: 128,
        cores: 1,
        ..RuntimeTables::default()
    };
    tables.stripe_for_cores(1);
    generate_with(&tables, &RtconfigExtras::default())
}

/// Pretty-print the facts-only config module for `tables`.
///
/// `tables.cores` must already reflect `PlacementTable.cores` (call
/// [`RuntimeTables::stripe_for_cores`] first). Emits `const N_CORES: usize =
/// <tables.cores>` with that exact spelling (decision 709 / 761).
///
/// Item E (decision 781): structured `TurnArea` / `GroupSlot` overlays.
/// Item F (decisions 790–792): `SCHED` stripe; child tag/payload fields;
/// match ladders + fixed stub pools from `tables.select_by_core` /
/// `drain_by_core` / `child_sites` (filled by `RuntimeWiring::derive`).
/// Item G (decisions 800–802): ring overlays + handle-identity ladders.
pub fn generate(tables: &RuntimeTables) -> String {
    let extras = extras_from_tables(tables);
    generate_with(tables, &extras)
}

/// Build [`RtconfigExtras`] from stamped `RuntimeTables` fields (shared by
/// dump `generate` and live reinject — decision 800).
pub fn extras_from_tables(tables: &RuntimeTables) -> RtconfigExtras {
    let placement = place_runtime_tables(RTDATA_BASE, tables);
    let mut rings = Vec::new();
    for (i, r) in tables.rings.iter().enumerate() {
        let addrs = placement
            .rings
            .get(i)
            .copied()
            .unwrap_or(crate::layout::RingAddrs {
                ring: RTDATA_BASE,
                head: RTDATA_BASE,
                tail: RTDATA_BASE,
                count: RTDATA_BASE,
            });
        let (target_handle, target_actor) = match r.kind {
            RingKind::Request => {
                let actor = r.actor.clone().unwrap_or_default();
                let handle = tables
                    .ring_target_handles
                    .get(i)
                    .copied()
                    .or_else(|| {
                        tables
                            .enqueue_handles
                            .iter()
                            .zip(tables.enqueue_actors.iter())
                            .find(|(_, n)| *n == &actor)
                            .map(|(h, _)| *h)
                    })
                    .unwrap_or(0);
                (Some(handle), Some(actor))
            }
            RingKind::Reply => (None, None),
        };
        rings.push(RingFact {
            kind: r.kind,
            src: r.src,
            dst: r.dst,
            capacity: r.capacity,
            slot_size: r.slot_size,
            ring_base: addrs.ring,
            head: addrs.head,
            tail: addrs.tail,
            count: addrs.count,
            target_handle,
            target_actor,
        });
    }
    RtconfigExtras {
        select_by_core: tables.select_by_core.clone(),
        drain_by_core: tables.drain_by_core.clone(),
        child_sites: tables
            .child_sites
            .iter()
            .map(|(callee_key, child_index, turn_index)| ChildSiteFact {
                callee_key: callee_key.clone(),
                child_index: *child_index,
                turn_index: *turn_index,
            })
            .collect(),
        rings,
        enqueue_handles: tables.enqueue_handles.clone(),
        enqueue_actors: tables.enqueue_actors.clone(),
    }
}

/// Pretty-print the facts-only config module for `tables` + item-F/G extras.
///
/// `tables.cores` must already reflect `PlacementTable.cores` (call
/// [`RuntimeTables::stripe_for_cores`] first). Emits `const N_CORES: usize =
/// <tables.cores>` with that exact spelling (decision 709 / 761).
///
/// Item E (decision 781): structured `TurnArea` / `GroupSlot` overlays.
/// Item F (decisions 790–792): `SchedCore` / `SCHED` for RR cursors;
/// child tag/payload fields; match ladders + placeholder stubs that layout
/// remaps onto `rt_select_and_run` / resume callees.
/// Item G (decisions 800–802): ring overlays + handle-identity / drain
/// lane ladders; enqueue stubs remapped to `rt_enqueue` until item J.
pub fn generate_with(tables: &RuntimeTables, extras: &RtconfigExtras) -> String {
    let placement = place_runtime_tables(RTDATA_BASE, tables);
    let n_turns_len = (tables.n_turns as usize).max(1);
    // Overlay stride is at least 0x48 so `ambient_group` at TURN_RECORD_SIZE
    // (0x40) always fits for batch-2 typecheck of deadline helpers. Live
    // reinject still requires `tables.turn_stride >= 0x48` so indexing
    // matches rtdata packing (decision 781 / 785). G adds `reply_tag` at
    // 0x38 (OFF_TURN_REPLY_TAG) so drain can write it through RT.turns.
    let turn_stride = if tables.turn_stride == 0 {
        128
    } else {
        (tables.turn_stride as usize).max(0x48)
    };
    let group_cap = tables.group_arena_capacity as usize;
    let group_slots = group_cap.max(1);
    // When the arena is empty, `place_runtime_tables` still returns a
    // `group_arena` cursor equal to the first ring's base (0-byte arena).
    // Overlay GROUPS at a non-colliding placeholder so RING*_DATA can own
    // the real ring addresses (decision 800) — same move as empty-sched.
    let group_addr = if group_cap == 0 {
        RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX - (RING_POOL_COUNT as u64) * 64 - 96
    } else {
        placement.group_arena
    };
    let n_cores = tables.cores;
    let ready_cap = tables.ready_queue_capacity as usize;
    let n_rings = extras.rings.len();

    // Flatten select actors for stub numbering (stable across cores).
    let mut select_flat: Vec<(usize, usize, String)> = Vec::new();
    for (core, actors) in extras.select_by_core.iter().enumerate() {
        for (slot, name) in actors.iter().enumerate() {
            select_flat.push((core, slot, name.clone()));
        }
    }
    let n_child = extras.child_sites.len();
    assert!(
        select_flat.len() <= SELECT_STUB_COUNT,
        "image needs {} select stubs; pool is {SELECT_STUB_COUNT} (decision 791)",
        select_flat.len()
    );
    assert!(
        n_child <= RESUME_STUB_COUNT,
        "image needs {n_child} resume stubs; pool is {RESUME_STUB_COUNT} (decision 791)"
    );
    assert!(
        n_rings <= RING_POOL_COUNT,
        "image needs {n_rings} rings; pool is {RING_POOL_COUNT} (decision 802)"
    );
    assert!(
        extras.enqueue_actors.len() <= ENQUEUE_STUB_COUNT,
        "image needs {} enqueue stubs; pool is {ENQUEUE_STUB_COUNT} (decision 802)",
        extras.enqueue_actors.len()
    );

    let mut out = String::new();
    out.push_str("module __image_runtime\n");
    out.push_str("\n");
    out.push_str("# generated by wrela; do not edit (dump --stage=rtconfig)\n");
    out.push_str("\n");
    push_const(&mut out, "N_CORES", n_cores);
    push_const(&mut out, "N_TURNS", tables.n_turns as usize);
    push_const(&mut out, "N_TURNS_LEN", n_turns_len);
    push_const(&mut out, "N_ACTORS", tables.actors.len());
    push_const(&mut out, "N_DRIVERS", tables.drivers.len());
    push_const(&mut out, "READY_QUEUE_CAPACITY", ready_cap);
    push_const(&mut out, "GROUP_ARENA_CAPACITY", group_cap);
    push_const(&mut out, "GROUP_SLOTS", group_slots);
    push_const(&mut out, "TURN_STRIDE", turn_stride);
    push_const(&mut out, "RTDATA_BYTES", tables.total_bytes as usize);
    push_const(&mut out, "N_CHILD_SITES", n_child);
    push_const(&mut out, "N_SELECT_STUBS", select_flat.len());
    push_const(&mut out, "SELECT_STUB_COUNT", SELECT_STUB_COUNT);
    push_const(&mut out, "RESUME_STUB_COUNT", RESUME_STUB_COUNT);
    push_const(&mut out, "N_RINGS", n_rings);
    push_const(&mut out, "RING_POOL_COUNT", RING_POOL_COUNT);
    push_const(&mut out, "ENQUEUE_STUB_COUNT", ENQUEUE_STUB_COUNT);
    push_const(&mut out, "NO_EDGE", NO_EDGE);
    out.push('\n');

    // Turn header. `reply` at +24 is OFF_TURN_REPLY; `reply_tag` at +0x38
    // is OFF_TURN_REPLY_TAG (drain writes both — decision 803).
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct TurnArea:\n");
    out.push_str("    busy: u64\n");
    out.push_str("    suspended: u64\n");
    out.push_str("    resume_ready: u64\n");
    out.push_str("    reply: u64\n");
    out.push_str("    @offset(0x38) reply_tag: u64\n");
    out.push_str("    @offset(0x40) ambient_group: u64\n");
    if turn_stride > 0x48 {
        out.push_str(&format!("    @offset({:#x}) _tail: u8\n", turn_stride - 1));
    }
    out.push('\n');

    // Group arena slot (GROUP_SLOT_SIZE == 96). join_waiter / owner_turn are
    // TurnIds (u32); children occupy +64..+96.
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct GroupSlot:\n");
    out.push_str("    in_use: u64\n");
    out.push_str("    capacity: u64\n");
    out.push_str("    active_children: u64\n");
    out.push_str("    deadline_ns: u64\n");
    out.push_str("    cancelled: u64\n");
    out.push_str("    parent: u64\n");
    out.push_str("    @offset(0x30) join_waiter: u32\n");
    out.push_str("    @offset(0x38) owner_turn: u32\n");
    out.push_str("    @offset(0x40) child0_tag: u64\n");
    out.push_str("    child0_payload: u64\n");
    out.push_str("    child1_tag: u64\n");
    out.push_str("    child1_payload: u64\n");
    out.push('\n');

    // Per-core ready queue + RR cursor, matching `place_runtime_tables`
    // after the turn array (decision 790).
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct SchedCore:\n");
    out.push_str("    ready: [u64; READY_QUEUE_CAPACITY]\n");
    out.push_str("    rr_cursor: u64\n");
    out.push('\n');

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct RuntimeTables:\n");
    out.push_str("    turns: [TurnArea; N_TURNS_LEN]\n");
    out.push('\n');
    out.push_str(&format!("@placed({RTDATA_BASE:#x})\n"));
    out.push_str("pub static RT: RuntimeTables\n");
    out.push('\n');

    let sched_base = if tables.n_turns == 0 {
        RTDATA_BASE + (n_turns_len as u64) * (turn_stride as u64)
    } else {
        match placement.rr_cursors.first() {
            Some(&rr) => rr - (ready_cap as u64) * 8,
            None => RTDATA_BASE + tables.n_turns * tables.turn_stride,
        }
    };
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct SchedStripe:\n");
    out.push_str("    cores: [SchedCore; N_CORES]\n");
    out.push('\n');
    out.push_str(&format!("@placed({sched_base:#x})\n"));
    out.push_str("pub static SCHED: SchedStripe\n");
    out.push('\n');

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct GroupArena:\n");
    out.push_str("    slots: [GroupSlot; GROUP_SLOTS]\n");
    out.push('\n');
    out.push_str(&format!("@placed({group_addr:#x})\n"));
    out.push_str("pub static GROUPS: GroupArena\n");
    out.push('\n');

    // Cross-core ring overlays (decision 800). Pool is fixed so runtime.wr
    // can import every RING*_CTL / RING*_DATA name; unused slots sit at a
    // non-colliding placeholder past the overlay turn array.
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct RingCtl:\n");
    out.push_str("    head: u64\n");
    out.push_str("    tail: u64\n");
    out.push_str("    count: u64\n");
    out.push('\n');
    let ring_placeholder =
        RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX - (RING_POOL_COUNT as u64) * 64;
    for i in 0..RING_POOL_COUNT {
        let (words, data_addr, ctl_addr) = if let Some(r) = extras.rings.get(i) {
            let words = ((r.capacity * r.slot_size) / 8).max(1) as usize;
            (words, r.ring_base, r.head)
        } else {
            (
                1usize,
                ring_placeholder + (i as u64) * 64,
                ring_placeholder + (i as u64) * 64 + 32,
            )
        };
        push_const(&mut out, &format!("RING{i}_WORDS"), words);
        out.push_str("@layout(runtime, endian=little)\n");
        out.push_str(&format!("struct Ring{i}Data:\n"));
        out.push_str(&format!("    words: [u64; RING{i}_WORDS]\n"));
        out.push('\n');
        out.push_str(&format!("@placed({data_addr:#x})\n"));
        out.push_str(&format!("pub static RING{i}_DATA: Ring{i}Data\n"));
        out.push('\n');
        out.push_str(&format!("@placed({ctl_addr:#x})\n"));
        out.push_str(&format!("pub static RING{i}_CTL: RingCtl\n"));
        out.push('\n');
    }

    // Fixed stub pools (decision 791 / 802).
    for i in 0..SELECT_STUB_COUNT {
        out.push_str(&format!("pub fn __select_{i}() -> u64:\n"));
        out.push_str("    return 0\n");
        out.push('\n');
    }
    for i in 0..RESUME_STUB_COUNT {
        out.push_str(&format!("pub fn __resume_{i}() -> u64:\n"));
        out.push_str("    return 0\n");
        out.push('\n');
    }
    for i in 0..ENQUEUE_STUB_COUNT {
        out.push_str(&format!(
            "pub fn __enqueue_{i}(method: u64, arg0: u64, arg1: u64, waker_turn: u64, waker_core: u64) -> u64:\n"
        ));
        out.push_str("    return 0\n");
        out.push('\n');
    }

    // --- match ladders (facts only; no if/while) ---------------------------
    out.push_str("pub fn __wrela_select_count(core: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, actors) in extras.select_by_core.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        out.push_str(&format!("            return {}\n", actors.len()));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_try_select(core: usize, slot: usize) -> u64:\n");
    out.push_str("    match core:\n");
    let mut stub_i = 0usize;
    for (core, actors) in extras.select_by_core.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        if actors.is_empty() {
            out.push_str("            return 0\n");
        } else {
            out.push_str("            match slot:\n");
            for slot in 0..actors.len() {
                out.push_str(&format!("                case {slot}:\n"));
                out.push_str(&format!("                    return __select_{stub_i}()\n"));
                stub_i += 1;
            }
            out.push_str("                case _:\n");
            out.push_str("                    return 0\n");
        }
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_resume_child(site: usize) -> u64:\n");
    out.push_str("    match site:\n");
    for i in 0..n_child {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return __resume_{i}()\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_child_turn_index(site: usize) -> usize:\n");
    out.push_str("    match site:\n");
    for (i, site) in extras.child_sites.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", site.turn_index));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_child_slot(site: usize) -> usize:\n");
    out.push_str("    match site:\n");
    for (i, site) in extras.child_sites.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", site.child_index));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    // --- item G: ring / handle-identity ladders (decision 801) ------------
    emit_ring_u64_ladder(&mut out, "capacity", extras, |r| r.capacity);
    emit_ring_usize_ladder(&mut out, "slot_words", extras, |r| {
        (r.slot_size / 8) as usize
    });
    emit_ring_usize_ladder(&mut out, "dst_core", extras, |r| r.dst);
    emit_ring_usize_ladder(&mut out, "src_core", extras, |r| r.src);
    emit_ring_usize_ladder(&mut out, "kind", extras, |r| match r.kind {
        RingKind::Request => 0,
        RingKind::Reply => 1,
    });
    emit_ring_usize_ladder(&mut out, "target_handle", extras, |r| {
        r.target_handle.unwrap_or(0) as usize
    });

    // CTL field accessors
    out.push_str("pub fn __wrela_ring_get_head(edge: usize) -> u64:\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return RING{i}_CTL.head\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_set_head(edge: usize, v: u64):\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            RING{i}_CTL.head = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_get_tail(edge: usize) -> u64:\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return RING{i}_CTL.tail\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_set_tail(edge: usize, v: u64):\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            RING{i}_CTL.tail = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_get_count(edge: usize) -> u64:\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return RING{i}_CTL.count\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_set_count(edge: usize, v: u64):\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            RING{i}_CTL.count = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_load_word(edge: usize, wi: usize) -> u64:\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return RING{i}_DATA.words[wi]\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_ring_store_word(edge: usize, wi: usize, v: u64):\n");
    out.push_str("    match edge:\n");
    for i in 0..RING_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            RING{i}_DATA.words[wi] = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    // Handle-identity xsend edge lookup (decision 801).
    out.push_str("pub fn __wrela_xsend_edge(handle: usize, src_core: usize) -> usize:\n");
    out.push_str("    match src_core:\n");
    let mut xsend_by_src: std::collections::BTreeMap<usize, Vec<(usize, usize)>> =
        std::collections::BTreeMap::new();
    for (ei, r) in extras.rings.iter().enumerate() {
        if r.kind == RingKind::Request {
            let h = r.target_handle.unwrap_or(0) as usize;
            xsend_by_src.entry(r.src).or_default().push((h, ei));
        }
    }
    for (src, arms) in &xsend_by_src {
        out.push_str(&format!("        case {src}:\n"));
        out.push_str("            match handle:\n");
        for (h, ei) in arms {
            out.push_str(&format!("                case {h}:\n"));
            out.push_str(&format!("                    return {ei}\n"));
        }
        out.push_str("                case _:\n");
        out.push_str(&format!("                    return {NO_EDGE}\n"));
    }
    out.push_str("        case _:\n");
    out.push_str(&format!("            return {NO_EDGE}\n"));
    out.push('\n');

    // xreply edge by (src, dst).
    out.push_str("pub fn __wrela_xreply_edge(src_core: usize, dst_core: usize) -> usize:\n");
    out.push_str("    match src_core:\n");
    let mut xreply_by_src: std::collections::BTreeMap<usize, Vec<(usize, usize)>> =
        std::collections::BTreeMap::new();
    for (ei, r) in extras.rings.iter().enumerate() {
        if r.kind == RingKind::Reply {
            xreply_by_src.entry(r.src).or_default().push((r.dst, ei));
        }
    }
    for (src, arms) in &xreply_by_src {
        out.push_str(&format!("        case {src}:\n"));
        out.push_str("            match dst_core:\n");
        for (dst, ei) in arms {
            out.push_str(&format!("                case {dst}:\n"));
            out.push_str(&format!("                    return {ei}\n"));
        }
        out.push_str("                case _:\n");
        out.push_str(&format!("                    return {NO_EDGE}\n"));
    }
    out.push_str("        case _:\n");
    out.push_str(&format!("            return {NO_EDGE}\n"));
    out.push('\n');

    // Per-core drain lane lists (reply first, then request — decision 803).
    let mut reply_by_dst: Vec<Vec<usize>> = vec![Vec::new(); n_cores];
    let mut request_by_dst: Vec<Vec<usize>> = vec![Vec::new(); n_cores];
    for (ei, r) in extras.rings.iter().enumerate() {
        if r.dst >= n_cores {
            continue;
        }
        match r.kind {
            RingKind::Reply => reply_by_dst[r.dst].push(ei),
            RingKind::Request => request_by_dst[r.dst].push(ei),
        }
    }
    out.push_str("pub fn __wrela_drain_reply_count(core: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, lanes) in reply_by_dst.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        out.push_str(&format!("            return {}\n", lanes.len()));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_drain_reply_edge(core: usize, slot: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, lanes) in reply_by_dst.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        if lanes.is_empty() {
            out.push_str(&format!("            return {NO_EDGE}\n"));
        } else {
            out.push_str("            match slot:\n");
            for (slot, ei) in lanes.iter().enumerate() {
                out.push_str(&format!("                case {slot}:\n"));
                out.push_str(&format!("                    return {ei}\n"));
            }
            out.push_str("                case _:\n");
            out.push_str(&format!("                    return {NO_EDGE}\n"));
        }
    }
    out.push_str("        case _:\n");
    out.push_str(&format!("            return {NO_EDGE}\n"));
    out.push('\n');

    out.push_str("pub fn __wrela_drain_request_count(core: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, lanes) in request_by_dst.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        out.push_str(&format!("            return {}\n", lanes.len()));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_drain_request_edge(core: usize, slot: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, lanes) in request_by_dst.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        if lanes.is_empty() {
            out.push_str(&format!("            return {NO_EDGE}\n"));
        } else {
            out.push_str("            match slot:\n");
            for (slot, ei) in lanes.iter().enumerate() {
                out.push_str(&format!("                case {slot}:\n"));
                out.push_str(&format!("                    return {ei}\n"));
            }
            out.push_str("                case _:\n");
            out.push_str(&format!("                    return {NO_EDGE}\n"));
        }
    }
    out.push_str("        case _:\n");
    out.push_str(&format!("            return {NO_EDGE}\n"));
    out.push('\n');

    // Enqueue by handle identity (decision 801).
    out.push_str(
        "pub fn __wrela_try_enqueue(handle: usize, method: u64, arg0: u64, arg1: u64, waker_turn: u64, waker_core: u64) -> u64:\n",
    );
    out.push_str("    match handle:\n");
    for (i, h) in extras.enqueue_handles.iter().enumerate() {
        out.push_str(&format!("        case {h}:\n"));
        out.push_str(&format!(
            "            return __enqueue_{i}(method, arg0, arg1, waker_turn, waker_core)\n"
        ));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 1\n");

    out
}

fn emit_ring_u64_ladder(
    out: &mut String,
    field: &str,
    extras: &RtconfigExtras,
    f: impl Fn(&RingFact) -> u64,
) {
    out.push_str(&format!(
        "pub fn __wrela_ring_{field}(edge: usize) -> u64:\n"
    ));
    out.push_str("    match edge:\n");
    for (i, r) in extras.rings.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", f(r)));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');
}

fn emit_ring_usize_ladder(
    out: &mut String,
    field: &str,
    extras: &RtconfigExtras,
    f: impl Fn(&RingFact) -> usize,
) {
    out.push_str(&format!(
        "pub fn __wrela_ring_{field}(edge: usize) -> usize:\n"
    ));
    out.push_str("    match edge:\n");
    for (i, r) in extras.rings.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", f(r)));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');
}

/// Call-key remaps from generated stubs onto ImageStatic / resume targets
/// (decision 791 / 802). Applied to reinjected runtime codegen before insert.
pub fn stub_call_remaps(extras: &RtconfigExtras) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stub_i = 0usize;
    for actors in &extras.select_by_core {
        for name in actors {
            out.push((
                format!("__select_{stub_i}"),
                crate::codegen::rt_select_and_run_symbol(name),
            ));
            stub_i += 1;
        }
    }
    for (i, site) in extras.child_sites.iter().enumerate() {
        out.push((format!("__resume_{i}"), site.callee_key.clone()));
    }
    for (i, name) in extras.enqueue_actors.iter().enumerate() {
        out.push((
            format!("__enqueue_{i}"),
            crate::codegen::rt_enqueue_symbol(name),
        ));
    }
    out
}

/// Rewrite `Reloc::Call` keys in `f` according to `remaps` (from→to).
pub fn remap_call_keys(f: &mut crate::codegen::CodegenFn, remaps: &[(String, String)]) {
    for r in &mut f.relocs {
        if let crate::codegen::Reloc::Call { key, .. } = r {
            if let Some((_, to)) = remaps.iter().find(|(from, _)| from == key) {
                *key = to.clone();
            }
        }
    }
}

fn push_const(out: &mut String, name: &str, value: usize) {
    out.push_str(&format!("pub const {name}: usize = {value}\n"));
}

/// Lex + parse generated text into a `Module` (ordinary front end).
pub fn parse_generated(text: &str) -> Result<Module, String> {
    let tokens = lexer::lex(text)
        .map_err(|e| format!("rtconfig lex: {} at {}:{}", e.message, e.line, e.col))?;
    parser::parse(tokens)
        .map_err(|e| format!("rtconfig parse: {} at {}:{}", e.message, e.line, e.col))
}

/// True when `text` contains a forbidden facts-only keyword as a whole word
/// (decision 702 / 763). Deliberately dumb: substring-in-comment would also
/// fail, which is fine — the generator never emits those words.
pub fn contains_forbidden_construct(text: &str) -> bool {
    for word in ["while", "for", "async", "@actor"] {
        if text
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '@')
            .any(|t| t == word)
        {
            return true;
        }
    }
    false
}

/// Insert `Input path=<generated> sha256=…` after the last existing
/// `Input path=` line in a rendered report, or after the quota block if
/// none. Stable across runs (decision 764).
pub fn insert_generated_input_line(report: &mut String, digest: &str) {
    let line = format!("  Input path={GENERATED_INPUT_PATH} sha256={digest}\n");
    let mut last_input_end: Option<usize> = None;
    for (idx, _) in report.match_indices("  Input path=") {
        let end = report[idx..]
            .find('\n')
            .map(|n| idx + n + 1)
            .unwrap_or(report.len());
        last_input_end = Some(end);
    }
    if let Some(at) = last_input_end {
        report.insert_str(at, &line);
        return;
    }
    let mut after_quota: Option<usize> = None;
    for (idx, _) in report.match_indices("  Quota ") {
        let end = report[idx..]
            .find('\n')
            .map(|n| idx + n + 1)
            .unwrap_or(report.len());
        after_quota = Some(end);
    }
    if let Some(at) = after_quota {
        report.insert_str(at, &line);
    } else {
        report.push_str(&line);
    }
}

/// Batch-2 front end (decision 704 / 723 / 765 / 780): parse generated text +
/// unstripped `runtime.wr`, run the ordinary `check_program_typed`.
pub fn typecheck_batch2(generated_text: &str) -> Result<(), String> {
    if contains_forbidden_construct(generated_text) {
        return Err(
            "error[build]: generated rtconfig contains a forbidden construct \
             (while/for/async/@actor); facts-only rule (plans/M11.md decision 702)"
                .to_string(),
        );
    }
    let gen_module = parse_generated(generated_text)?;
    let (runtime_key, runtime_loaded) =
        crate::loader::load_runtime_module_with_image_runtime_import().map_err(|e| match e {
            crate::loader::LoadError::Lex(err) => {
                format!("error[lex]: {} at {}:{}", err.message, err.line, err.col)
            }
            crate::loader::LoadError::Parse(err) => {
                format!("error[parse]: {} at {}:{}", err.message, err.line, err.col)
            }
            crate::loader::LoadError::Build(err) => format!(
                "error[{}]: {} at {}:{}",
                err.category, err.message, err.line, err.col
            ),
        })?;
    let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let mut modules = std::collections::BTreeMap::new();
    modules.insert(gen_key.clone(), gen_module);
    modules.insert(runtime_key.clone(), runtime_loaded.module);
    let mut paths = std::collections::BTreeMap::new();
    paths.insert(gen_key, GENERATED_INPUT_PATH.to_string());
    paths.insert(runtime_key, runtime_loaded.file.display().to_string());
    crate::sema::check_program_typed(&modules, &paths).map_err(|e| {
        format!(
            "error[{}]: {} at {}:{}",
            e.category, e.message, e.line, e.col
        )
    })?;
    Ok(())
}

/// Generate + batch-2 typecheck for a laid-out image's runtime tables.
/// Returns the generated source text.
pub fn generate_and_typecheck(tables: &RuntimeTables) -> Result<String, String> {
    let text = generate(tables);
    typecheck_batch2(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::RuntimeTables;

    fn sample_tables(cores: usize) -> RuntimeTables {
        // Consistent empty turn set so `place_runtime_tables` (used by
        // `generate` for GROUPS @placed) does not trip its n_turns assert.
        let mut t = RuntimeTables {
            n_turns: 0,
            turn_stride: 0,
            ready_queue_capacity: 4,
            group_arena_capacity: 0,
            total_bytes: 1024,
            cores: 1,
            ..RuntimeTables::default()
        };
        t.stripe_for_cores(cores);
        t
    }

    #[test]
    fn n_cores_spelling_and_values() {
        let one = generate(&sample_tables(1));
        assert!(
            one.contains("pub const N_CORES: usize = 1\n"),
            "single-core must spell N_CORES exactly; got:\n{one}"
        );
        let three = generate(&sample_tables(3));
        assert!(
            three.contains("pub const N_CORES: usize = 3\n"),
            "cross-core must spell N_CORES exactly; got:\n{three}"
        );
    }

    #[test]
    fn placed_uses_rtdata_base_literal() {
        let text = generate(&sample_tables(1));
        assert!(
            text.contains(&format!("@placed({RTDATA_BASE:#x})\n")),
            "expected @placed at RTDATA_BASE; got:\n{text}"
        );
        assert!(!text.contains("@placed(RTDATA_BASE)"));
        assert!(
            text.contains("pub static RT: RuntimeTables\n"),
            "expected RT static; got:\n{text}"
        );
        assert!(
            text.contains("pub static GROUPS: GroupArena\n"),
            "expected GROUPS static; got:\n{text}"
        );
    }

    #[test]
    fn facts_only_forbids_control_and_actors() {
        let text = generate(&sample_tables(1));
        assert!(
            !contains_forbidden_construct(&text),
            "generator emitted a forbidden construct:\n{text}"
        );
        assert!(contains_forbidden_construct("while true:\n    return\n"));
        assert!(contains_forbidden_construct("for x in xs:\n    return\n"));
        assert!(contains_forbidden_construct("async fn f():\n    return\n"));
        assert!(contains_forbidden_construct(
            "@actor\nstruct A:\n    x: u64\n"
        ));
    }

    #[test]
    fn generate_is_deterministic_across_two_runs() {
        let tables = sample_tables(1);
        let a = generate(&tables);
        let b = generate(&tables);
        assert_eq!(a, b);
    }

    #[test]
    fn generated_text_parses() {
        let text = generate(&sample_tables(2));
        let module = parse_generated(&text).expect("parse");
        assert_eq!(module.path, vec!["__image_runtime".to_string()]);
    }

    #[test]
    fn structured_fields_present() {
        let mut t = sample_tables(1);
        t.group_arena_capacity = 1;
        t.total_bytes = 2048;
        let text = generate(&t);
        assert!(text.contains("struct TurnArea:\n"));
        assert!(text.contains("struct GroupSlot:\n"));
        assert!(text.contains("struct SchedCore:\n"));
        assert!(text.contains("struct SchedStripe:\n"));
        assert!(text.contains("pub const GROUP_SLOTS: usize = 1\n"));
        assert!(text.contains("turns: [TurnArea; N_TURNS_LEN]\n"));
        assert!(text.contains("cores: [SchedCore; N_CORES]\n"));
        assert!(text.contains("pub static SCHED: SchedStripe\n"));
        assert!(text.contains("slots: [GroupSlot; GROUP_SLOTS]\n"));
        assert!(text.contains("pub const N_CHILD_SITES: usize = 0\n"));
    }
}

#[cfg(test)]
mod typecheck_live {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn imported_consts_visible_to_eval_and_lower() {
        let text = stub_text();
        let gen_mod = parse_generated(&text).unwrap();
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(crate::loader::LoadError::Lex(e)) => panic!("runtime lex: {}", e.message),
            Err(crate::loader::LoadError::Parse(e)) => panic!("runtime parse: {}", e.message),
            Err(crate::loader::LoadError::Build(e)) => panic!("runtime build: {}", e.message),
        };
        let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let mut modules = BTreeMap::new();
        modules.insert(gen_key.clone(), gen_mod);
        modules.insert(runtime_key.clone(), runtime_loaded.module);
        let mut paths = BTreeMap::new();
        paths.insert(gen_key.clone(), GENERATED_INPUT_PATH.to_string());
        paths.insert(runtime_key.clone(), "runtime.wr".into());
        let programs = crate::sema::check_program_typed(&modules, &paths).expect("typed");
        let runtime = programs
            .iter()
            .find(|(k, _)| k.as_slice() == ["core", "runtime"])
            .map(|(_, p)| p)
            .expect("runtime program");
        assert!(
            runtime.imported.consts.contains_key("GROUP_ARENA_CAPACITY"),
            "missing imported const; keys={:?}",
            runtime.imported.consts.keys().collect::<Vec<_>>()
        );
        crate::eval::interp::eval_const(runtime, "GROUP_ARENA_CAPACITY")
            .expect("eval imported const");
        // Deadline helpers are reinjected only when a group arena exists
        // (not always force-rooted); seed them for this lower coverage.
        let mut only = crate::lower::guest_reachable_keys_closure(
            &{
                let map: BTreeMap<String, _> = programs
                    .iter()
                    .map(|(k, p)| (k.join("."), p.clone()))
                    .collect();
                map
            },
            &crate::lower::LowerOpts::default(),
        );
        only.insert("__wrela_deadline_poll".into());
        only.insert("__wrela_deadline_scan".into());
        assert!(only.contains("__wrela_deadline_poll"), "reachable={only:?}");
        let opts = crate::lower::LowerOpts {
            emit_comptime_tests: false,
            only: Some(only),
        };
        crate::lower::lower_program_with(runtime, &opts).expect("lower runtime");
    }

    #[test]
    fn turn_area_sized_to_stride() {
        let mut t = RuntimeTables {
            n_turns: 3,
            turn_stride: 1024,
            ready_queue_capacity: 2,
            group_arena_capacity: 1,
            total_bytes: 3352,
            cores: 1,
            ..RuntimeTables::default()
        };
        t.actors.push(crate::layout::ActorRuntimeLayout {
            name: "Counter".into(),
            state_size: 8,
            mailbox_capacity: 4,
            slot_size: 32,
            frame_size: 128,
        });
        t.free_turns = vec![("worker".into(), 128), ("test".into(), 128)];
        t.stripe_for_cores(1);
        let text = generate(&t);
        assert!(
            text.contains("@offset(0x3ff) _tail: u8"),
            "generated text missing 0x3ff tail: {text}"
        );
        let gen_mod = parse_generated(&text).unwrap();
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(crate::loader::LoadError::Lex(e)) => panic!("{}", e.message),
            Err(crate::loader::LoadError::Parse(e)) => panic!("{}", e.message),
            Err(crate::loader::LoadError::Build(e)) => panic!("{}", e.message),
        };
        let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let mut modules = BTreeMap::new();
        modules.insert(gen_key.clone(), gen_mod);
        modules.insert(runtime_key.clone(), runtime_loaded.module);
        let mut paths = BTreeMap::new();
        paths.insert(gen_key, GENERATED_INPUT_PATH.to_string());
        paths.insert(runtime_key, "runtime.wr".into());
        let programs = crate::sema::check_program_typed(&modules, &paths).expect("typed");
        let image_rt = programs
            .iter()
            .find(|(k, _)| k.as_slice() == ["core", "__image_runtime"])
            .map(|(_, p)| p)
            .expect("gen");
        let turn = image_rt
            .layouts
            .iter()
            .find(|l| l.name == "TurnArea")
            .expect("TurnArea layout");
        assert_eq!(turn.size, Some(1024), "TurnArea layout size");
        let rt = image_rt
            .layouts
            .iter()
            .find(|l| l.name == "RuntimeTables")
            .expect("RuntimeTables");
        assert_eq!(rt.size, Some(3072), "RuntimeTables = 3 * 1024 turns");
    }
}
