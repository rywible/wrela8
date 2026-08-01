use crate::layout::{RingKind, RuntimeTables, place_runtime_tables};
use crate::syntax::ast::Module;
use crate::syntax::{lexer, parser};
use wrela_machine::layout::RTDATA_BASE;

#[derive(Debug, Clone)]
pub struct ChildSiteFact {
    pub callee_key: String,
    pub child_index: usize,
    pub turn_index: usize,
}

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
    pub target_handle: Option<u64>,
    pub target_actor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MailboxFact {
    pub name: String,
    pub capacity: u64,
    pub slot_size: u64,
    pub frame_size: u64,
    pub state: u64,
    pub ring: u64,
    pub head: u64,
    pub turn_index: usize,
    pub core: usize,
    pub methods: Vec<MethodFact>,
}

#[derive(Debug, Clone)]
pub struct MethodFact {
    pub key: String,
    pub is_async: bool,
    pub reply_is_aggregate: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RtconfigExtras {
    pub select_by_core: Vec<Vec<String>>,
    pub drain_by_core: Vec<bool>,
    pub child_sites: Vec<ChildSiteFact>,
    pub rings: Vec<RingFact>,
    pub enqueue_handles: Vec<u64>,
    pub enqueue_actors: Vec<String>,
    pub mailboxes: Vec<MailboxFact>,
    pub init_slots: Vec<(u64, u64)>,
    pub n_boot_calls: usize,
    pub irq_vector_bits: Vec<u64>,
    pub wake_pending_addrs: Vec<u64>,
    pub tests: Vec<TestRunnerFact>,
    pub has_boot_init: bool,
}

#[derive(Debug, Clone)]
pub struct TestRunnerFact {
    pub name: String,
    pub is_async: bool,
    pub turn_index: usize,
}

pub const MODULE_PATH: &[&str] = &["__image_runtime"];

pub const MODULE_ADDR: &str = "core.__image_runtime";

pub const GENERATED_INPUT_PATH: &str = "<generated>";

pub const DUMP_STAGE: &str = "rtconfig";

pub const SELECT_STUB_COUNT: usize = 32;
pub const RESUME_STUB_COUNT: usize = 16;
pub const RING_POOL_COUNT: usize = 8;
pub const ENQUEUE_STUB_COUNT: usize = 32;
pub const MB_POOL_COUNT: usize = 32;
pub const METHOD_CALL_POOL_COUNT: usize = 128;
pub const BLOCK_POOL_COUNT: usize = 3072;
pub const BLOCK_BOUND_PRINT_PAIRS: usize = 128;
pub const BOOT_CALL_POOL_COUNT: usize = 32;
pub const INIT_SPAN_POOL_COUNT: usize = 8;
pub const IRQ_CALL_POOL_COUNT: usize = 8;
pub const WAKE_CALL_POOL_COUNT: usize = 8;
pub const TEST_CALL_POOL_COUNT: usize = 16;
pub const NO_EDGE: usize = 255;

pub fn stub_text() -> String {
    let mut tables = RuntimeTables {
        n_turns: 0,
        turn_stride: 0,
        ready_queue_capacity: 1,
        group_arena_capacity: 0,
        group_max_children: crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        total_bytes: 128,
        cores: 1,
        ..RuntimeTables::default()
    };
    tables.stripe_for_cores(1);
    generate_with(&tables, &RtconfigExtras::default())
        .expect("rtconfig stub stays in pool ceilings")
}

pub fn generate(tables: &RuntimeTables) -> Result<String, String> {
    let extras = extras_from_tables(tables)?;
    generate_with(tables, &extras)
}

pub fn extras_from_tables(tables: &RuntimeTables) -> Result<RtconfigExtras, String> {
    let placement = place_runtime_tables(RTDATA_BASE, tables);
    let mut init_slots: Vec<(u64, u64)> = Vec::new();
    for (a, addrs) in tables.actors.iter().zip(placement.actors.iter()) {
        init_slots.push((addrs.state, a.state_size / 8));
    }
    for (d, &state) in tables.drivers.iter().zip(placement.drivers.iter()) {
        init_slots.push((state, d.state_size / 8));
    }
    let mut rings = Vec::new();
    for (i, r) in tables.rings.iter().enumerate() {
        let addrs = placement.rings.get(i).copied().ok_or_else(|| {
            format!("rtconfig: ring {i} has no placement (tables/placement disagree)")
        })?;
        let (target_handle, target_actor) = match r.kind {
            RingKind::Request => {
                let actor = r.actor.clone().ok_or_else(|| {
                    format!("rtconfig: request ring {i} has no target actor name")
                })?;
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
                    .ok_or_else(|| {
                        format!("rtconfig: request ring {i} target `{actor}` has no enqueue handle")
                    })?;
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
    let mut mailboxes = Vec::new();
    for (i, name) in tables.enqueue_actors.iter().enumerate() {
        let (capacity, slot_size, frame_size, state, ring, head, turn_index) =
            if let Some((ai, a)) = tables
                .actors
                .iter()
                .enumerate()
                .find(|(_, a)| a.name == *name)
            {
                let addrs = placement
                    .actors
                    .get(ai)
                    .copied()
                    .ok_or_else(|| format!("rtconfig: actor `{name}` has no placement"))?;
                (
                    a.mailbox_capacity,
                    a.slot_size,
                    a.frame_size,
                    addrs.state,
                    addrs.ring,
                    addrs.head,
                    ai,
                )
            } else {
                let (di, d) = tables
                    .drivers
                    .iter()
                    .enumerate()
                    .find(|(_, d)| d.name == *name && d.mailbox.is_some())
                    .ok_or_else(|| {
                        format!(
                            "rtconfig: enqueue root `{name}` is neither an actor nor a \
                             messageable driver"
                        )
                    })?;
                let mb = d
                    .mailbox
                    .as_ref()
                    .ok_or_else(|| format!("rtconfig: driver `{name}` has no mailbox"))?;
                let addrs = placement
                    .driver_mailboxes
                    .get(&di)
                    .copied()
                    .ok_or_else(|| format!("rtconfig: driver mailbox `{name}` has no placement"))?;
                let turn_index = tables.actors.len()
                    + tables
                        .drivers
                        .iter()
                        .take(di)
                        .filter(|dd| dd.mailbox.is_some())
                        .count();
                (
                    mb.capacity,
                    mb.slot_size,
                    mb.frame_size,
                    addrs.state,
                    addrs.ring,
                    addrs.head,
                    turn_index,
                )
            };
        let methods = tables
            .root_methods
            .get(i)
            .map(|ms| {
                ms.iter()
                    .map(|(key, is_async, agg)| MethodFact {
                        key: key.clone(),
                        is_async: *is_async,
                        reply_is_aggregate: *agg,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let core = tables.root_cores.get(i).copied().unwrap_or(0);
        mailboxes.push(MailboxFact {
            name: name.clone(),
            capacity,
            slot_size,
            frame_size,
            state,
            ring,
            head,
            turn_index,
            core,
            methods,
        });
    }
    Ok(RtconfigExtras {
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
        mailboxes,
        init_slots,
        n_boot_calls: tables.n_boot_calls,
        irq_vector_bits: tables.irq_vector_bits.clone(),
        wake_pending_addrs: tables.wake_pending_addrs.clone(),
        tests: Vec::new(),
        has_boot_init: false,
    })
}

pub fn live_init_span_count(tables: &RuntimeTables) -> Result<usize, String> {
    let extras = extras_from_tables(tables)?;
    Ok(coalesce_init_spans(&extras.init_slots).len())
}

fn coalesce_init_spans(slots: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut live: Vec<(u64, u64)> = slots
        .iter()
        .copied()
        .filter(|&(_, nwords)| nwords > 0)
        .collect();
    live.sort_by_key(|&(addr, _)| addr);
    let mut spans: Vec<(u64, u64)> = Vec::new();
    for (addr, nwords) in live {
        if let Some(last) = spans.last_mut() {
            if last.0 + last.1 * 8 == addr {
                last.1 += nwords;
                continue;
            }
        }
        spans.push((addr, nwords));
    }
    spans
}

pub fn generate_with(tables: &RuntimeTables, extras: &RtconfigExtras) -> Result<String, String> {
    let placement = place_runtime_tables(RTDATA_BASE, tables);
    let init_spans = coalesce_init_spans(&extras.init_slots);
    let n_turns_len = (tables.n_turns as usize).max(1);
    let turn_stride = if tables.turn_stride == 0 {
        128
    } else {
        (tables.turn_stride as usize).max(0x48)
    };
    let group_cap = tables.group_arena_capacity as usize;
    let group_slots = group_cap.max(1);
    let max_children = tables
        .group_max_children
        .max(crate::codegen::GROUP_MAX_CHILDREN_FLOOR);
    let group_slot_size = crate::codegen::group_slot_size(max_children);
    let n_rings = extras.rings.len();
    let n_wake = extras.wake_pending_addrs.len();
    let rings_high_reserve: u64 = if n_rings == 0 { 24 + 8 } else { 0 };
    let wake_high_reserve: u64 = if n_wake == 0 { 8 } else { 0 };
    let group_addr = if group_cap == 0 {
        RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX
            - rings_high_reserve
            - wake_high_reserve
            - (MB_POOL_COUNT as u64) * 64
            - group_slot_size
    } else {
        placement.group_arena
    };
    let n_cores = tables.cores;
    let ready_cap = tables.ready_queue_capacity as usize;

    let mut select_flat: Vec<(usize, usize, String)> = Vec::new();
    for (core, actors) in extras.select_by_core.iter().enumerate() {
        for (slot, name) in actors.iter().enumerate() {
            select_flat.push((core, slot, name.clone()));
        }
    }
    let n_child = extras.child_sites.len();
    if select_flat.len() > SELECT_STUB_COUNT {
        return Err(format!(
            "image needs {} select stubs; pool is {SELECT_STUB_COUNT}",
            select_flat.len()
        ));
    }
    if n_child > RESUME_STUB_COUNT {
        return Err(format!(
            "image needs {n_child} resume stubs; pool is {RESUME_STUB_COUNT}"
        ));
    }
    if n_rings > RING_POOL_COUNT {
        return Err(format!(
            "image needs {n_rings} rings; pool is {RING_POOL_COUNT}"
        ));
    }
    if extras.enqueue_actors.len() > ENQUEUE_STUB_COUNT {
        return Err(format!(
            "image needs {} enqueue stubs; pool is {ENQUEUE_STUB_COUNT}",
            extras.enqueue_actors.len()
        ));
    }
    if extras.mailboxes.len() > MB_POOL_COUNT {
        return Err(format!(
            "image needs {} mailboxes; pool is {MB_POOL_COUNT}",
            extras.mailboxes.len()
        ));
    }
    let n_methods: usize = extras.mailboxes.iter().map(|m| m.methods.len()).sum();
    if n_methods > METHOD_CALL_POOL_COUNT {
        return Err(format!(
            "image needs {n_methods} method stubs; pool is {METHOD_CALL_POOL_COUNT}"
        ));
    }
    if init_spans.len() > INIT_SPAN_POOL_COUNT {
        return Err(format!(
            "image needs {} init spans; pool is {INIT_SPAN_POOL_COUNT}",
            init_spans.len()
        ));
    }
    if extras.n_boot_calls > BOOT_CALL_POOL_COUNT {
        return Err(format!(
            "image needs {} boot calls; pool is {BOOT_CALL_POOL_COUNT}",
            extras.n_boot_calls
        ));
    }
    if extras.irq_vector_bits.len() > IRQ_CALL_POOL_COUNT {
        return Err(format!(
            "image needs {} IRQ stubs; pool is {IRQ_CALL_POOL_COUNT}",
            extras.irq_vector_bits.len()
        ));
    }
    if extras.wake_pending_addrs.len() > WAKE_CALL_POOL_COUNT {
        return Err(format!(
            "image needs {} wake stubs; pool is {WAKE_CALL_POOL_COUNT}",
            extras.wake_pending_addrs.len()
        ));
    }

    let mut out = String::new();
    out.push_str("module __image_runtime\n");
    out.push_str("\n");
    out.push_str("# generated by wrela; do not edit (dump --stage=rtconfig)\n");
    out.push_str("\n");
    push_const(&mut out, "N_CORES", n_cores);
    push_const(&mut out, "CORE_SLOTS", wrela_machine::CORE_SLOTS);
    push_const(
        &mut out,
        "MACHINE_INFO_BASE",
        wrela_machine::layout::MACHINE_INFO_BASE as usize,
    );
    push_const(
        &mut out,
        "CONSOLE_RING_BASE",
        wrela_machine::console::RING_BASE as usize,
    );
    push_const(
        &mut out,
        "CONSOLE_DATA_BASE",
        wrela_machine::console::DATA_BASE as usize,
    );
    push_const(&mut out, "N_TURNS", tables.n_turns as usize);
    push_const(&mut out, "N_TURNS_LEN", n_turns_len);
    push_const(&mut out, "N_ACTORS", tables.actors.len());
    push_const(&mut out, "N_DRIVERS", tables.drivers.len());
    push_const(&mut out, "READY_QUEUE_CAPACITY", ready_cap);
    push_const(&mut out, "GROUP_ARENA_CAPACITY", group_cap);
    push_const(&mut out, "GROUP_SLOTS", group_slots);
    push_const(&mut out, "GROUP_MAX_CHILDREN", max_children);
    push_const(&mut out, "GROUP_SLOT_SIZE", group_slot_size as usize);
    push_const(&mut out, "TURN_STRIDE", turn_stride);
    push_const(&mut out, "RTDATA_BYTES", tables.total_bytes as usize);
    push_const(&mut out, "N_CHILD_SITES", n_child);
    push_const(&mut out, "N_SELECT_STUBS", select_flat.len());
    push_const(&mut out, "SELECT_STUB_COUNT", SELECT_STUB_COUNT);
    push_const(&mut out, "RESUME_STUB_COUNT", RESUME_STUB_COUNT);
    push_const(&mut out, "N_RINGS", n_rings);
    push_const(&mut out, "RING_POOL_COUNT", RING_POOL_COUNT);
    push_const(&mut out, "ENQUEUE_STUB_COUNT", ENQUEUE_STUB_COUNT);
    push_const(&mut out, "N_MAILBOXES", extras.mailboxes.len());
    push_const(&mut out, "MB_POOL_COUNT", MB_POOL_COUNT);
    push_const(&mut out, "METHOD_CALL_POOL_COUNT", METHOD_CALL_POOL_COUNT);
    push_const(&mut out, "BLOCK_POOL_COUNT", BLOCK_POOL_COUNT);
    push_const(&mut out, "BLOCK_BOUND_PRINT_PAIRS", BLOCK_BOUND_PRINT_PAIRS);
    push_const(&mut out, "N_METHODS", n_methods);
    push_const(&mut out, "TURNS_BASE", RTDATA_BASE as usize);
    push_const(&mut out, "N_INIT_SLOTS", init_spans.len());
    push_const(&mut out, "INIT_SPAN_POOL_COUNT", INIT_SPAN_POOL_COUNT);
    push_const(&mut out, "N_BOOT_CALLS", extras.n_boot_calls);
    push_const(&mut out, "BOOT_CALL_POOL_COUNT", BOOT_CALL_POOL_COUNT);
    push_const(&mut out, "N_IRQ_VECTORS", extras.irq_vector_bits.len());
    push_const(&mut out, "IRQ_CALL_POOL_COUNT", IRQ_CALL_POOL_COUNT);
    push_const(&mut out, "N_WAKE_DRAINS", n_wake);
    let n_wake_len = n_wake.max(1);
    push_const(&mut out, "N_WAKE_DRAINS_LEN", n_wake_len);
    push_const(&mut out, "WAKE_CALL_POOL_COUNT", WAKE_CALL_POOL_COUNT);
    let checkpoint_simple =
        extras.irq_vector_bits.is_empty() && extras.wake_pending_addrs.is_empty();
    out.push_str(&format!(
        "pub const CHECKPOINT_SIMPLE: bool = {}\n",
        if checkpoint_simple { "true" } else { "false" }
    ));
    push_const(&mut out, "NO_EDGE", NO_EDGE);
    out.push('\n');

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct TurnArea:\n");
    out.push_str("    busy: u64\n");
    out.push_str("    suspended: u64\n");
    out.push_str("    resume_ready: u64\n");
    out.push_str("    reply: u64\n");
    out.push_str("    @offset(0x20) waker_turn: u32\n");
    out.push_str("    waker_core: u32\n");
    out.push_str("    cur_method: u64\n");
    out.push_str("    reply_slot_turn: u32\n");
    out.push_str("    reply_slot_off: u32\n");
    out.push_str("    @offset(0x38) reply_tag: u64\n");
    out.push_str("    @offset(0x40) ambient_group: u64\n");
    out.push_str("    lineage_deadline: u64\n");
    if turn_stride > 0x50 {
        out.push_str(&format!("    @offset({:#x}) _tail: u8\n", turn_stride - 1));
    }
    out.push('\n');

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
    for c in 0..max_children {
        if c == 0 {
            out.push_str("    @offset(0x40) child0_tag: u64\n");
            out.push_str("    child0_payload: u64\n");
        } else {
            out.push_str(&format!("    child{c}_tag: u64\n"));
            out.push_str(&format!("    child{c}_payload: u64\n"));
        }
    }
    out.push('\n');

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct SchedCore:\n");
    out.push_str("    ready: [u64; READY_QUEUE_CAPACITY]\n");
    out.push_str("    rr_cursor: u64\n");
    out.push('\n');

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct RuntimeTables:\n");
    out.push_str("    turns: [TurnArea; N_TURNS_LEN]\n");
    out.push('\n');
    let rt_addr = if tables.n_turns == 0 {
        RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX
            - rings_high_reserve
            - wake_high_reserve
            - (MB_POOL_COUNT as u64) * 64
            - group_slot_size
            - (INIT_SPAN_POOL_COUNT as u64) * 8
            - (n_turns_len as u64) * (turn_stride as u64)
    } else {
        RTDATA_BASE
    };
    out.push_str(&format!("@placed({rt_addr:#x})\n"));
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

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct RingCtl:\n");
    out.push_str("    head: u64\n");
    out.push_str("    tail: u64\n");
    out.push_str("    count: u64\n");
    out.push('\n');
    let n_rings_len = n_rings.max(1);
    let ring_stride_words = if n_rings == 0 {
        1usize
    } else {
        extras
            .rings
            .iter()
            .map(|r| ((r.capacity * r.slot_size) / 8) as usize)
            .max()
            .unwrap_or(1)
            .max(1)
    };
    push_const(&mut out, "N_RINGS_LEN", n_rings_len);
    push_const(&mut out, "RING_STRIDE_WORDS", ring_stride_words);
    let (rings_ctl_addr, rings_data_addr) = if n_rings == 0 {
        let ph = RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX - rings_high_reserve;
        (ph, ph + 24)
    } else {
        let first = &extras.rings[0];
        (first.head, first.ring_base)
    };
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct RingsCtl:\n");
    out.push_str("    edges: [RingCtl; N_RINGS_LEN]\n");
    out.push('\n');
    out.push_str(&format!("@placed({rings_ctl_addr:#x})\n"));
    out.push_str("pub static RINGS_CTL: RingsCtl\n");
    out.push('\n');
    let rings_data_words = n_rings_len * ring_stride_words;
    push_const(&mut out, "RINGS_DATA_WORDS", rings_data_words);
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct RingsData:\n");
    out.push_str("    words: [u64; RINGS_DATA_WORDS]\n");
    out.push('\n');
    out.push_str(&format!("@placed({rings_data_addr:#x})\n"));
    out.push_str("pub static RINGS_DATA: RingsData\n");
    out.push('\n');

    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct MbCtl:\n");
    out.push_str("    head: u64\n");
    out.push_str("    tail: u64\n");
    out.push_str("    count: u64\n");
    out.push('\n');
    let mb_placeholder = RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX
        - rings_high_reserve
        - wake_high_reserve
        - (MB_POOL_COUNT as u64) * 64;
    for i in 0..MB_POOL_COUNT {
        let (words, data_addr, ctl_addr) = if let Some(m) = extras.mailboxes.get(i) {
            let words = ((m.capacity * m.slot_size) / 8).max(1) as usize;
            (words, m.ring, m.head)
        } else {
            (
                1usize,
                mb_placeholder + (i as u64) * 64,
                mb_placeholder + (i as u64) * 64 + 32,
            )
        };
        push_const(&mut out, &format!("MB{i}_WORDS"), words);
        out.push_str("@layout(runtime, endian=little)\n");
        out.push_str(&format!("struct Mb{i}Data:\n"));
        out.push_str(&format!("    words: [u64; MB{i}_WORDS]\n"));
        out.push('\n');
        out.push_str(&format!("@placed({data_addr:#x})\n"));
        out.push_str(&format!("pub static MB{i}_DATA: Mb{i}Data\n"));
        out.push('\n');
        out.push_str(&format!("@placed({ctl_addr:#x})\n"));
        out.push_str(&format!("pub static MB{i}_CTL: MbCtl\n"));
        out.push('\n');
    }

    for i in 0..RESUME_STUB_COUNT {
        out.push_str(&format!("pub fn __resume_{i}() -> u64:\n"));
        out.push_str("    return 0\n");
        out.push('\n');
    }
    for i in 0..METHOD_CALL_POOL_COUNT {
        out.push_str(&format!(
            "pub fn __method_{i}(arg0: u64, arg1: u64, stage: u64) -> u64:\n"
        ));
        out.push_str("    return 0\n");
        out.push('\n');
    }
    for i in 0..BOOT_CALL_POOL_COUNT {
        out.push_str(&format!("pub fn __boot_call_{i}():\n"));
        out.push_str("    return\n");
        out.push('\n');
    }
    for i in 0..IRQ_CALL_POOL_COUNT {
        out.push_str(&format!("pub fn __irq_call_{i}():\n"));
        out.push_str("    return\n");
        out.push('\n');
    }
    for i in 0..WAKE_CALL_POOL_COUNT {
        out.push_str(&format!("pub fn __wake_call_{i}():\n"));
        out.push_str("    return\n");
        out.push('\n');
    }

    if n_cores > 1 {
        out.push_str("pub fn __wrela_secondary_entry_body(core: usize):\n");
        out.push_str("    return\n");
        out.push('\n');
        for k in 1..n_cores {
            out.push_str(&format!("pub fn __wrela_secondary_entry_{k}():\n"));
            out.push_str(&format!("    __wrela_secondary_entry_body({k})\n"));
            out.push_str("    return\n");
            out.push('\n');
        }
    }

    let init_placeholder_base = RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX
        - rings_high_reserve
        - wake_high_reserve
        - (MB_POOL_COUNT as u64) * 64
        - group_slot_size
        - (INIT_SPAN_POOL_COUNT as u64) * 8;
    for i in 0..INIT_SPAN_POOL_COUNT {
        let (addr, nwords) = init_spans
            .get(i)
            .copied()
            .unwrap_or((init_placeholder_base + (i as u64) * 8, 1));
        out.push_str("@layout(runtime, endian=little)\n");
        out.push_str(&format!("struct InitSpan{i}Words:\n"));
        out.push_str(&format!("    words: [u64; {nwords}]\n"));
        out.push('\n');
        out.push_str(&format!("@placed({addr:#x})\n"));
        out.push_str(&format!("pub static INIT_SPAN{i}: InitSpan{i}Words\n"));
        out.push('\n');
    }

    let wake_addr = if n_wake == 0 {
        RTDATA_BASE + wrela_machine::layout::RTDATA_SIZE_MAX
            - rings_high_reserve
            - wake_high_reserve
    } else {
        extras.wake_pending_addrs[0]
    };
    out.push_str("@layout(runtime, endian=little)\n");
    out.push_str("struct WakeTables:\n");
    out.push_str("    wake_pending: [u64; N_WAKE_DRAINS_LEN]\n");
    out.push('\n');
    out.push_str(&format!("@placed({wake_addr:#x})\n"));
    out.push_str("pub static WAKE: WakeTables\n");
    out.push('\n');

    out.push_str("pub fn __wrela_select_count(core: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, actors) in extras.select_by_core.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        out.push_str(&format!("            return {}\n", actors.len()));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_select_root(core: usize, slot: usize) -> usize:\n");
    out.push_str("    match core:\n");
    for (core, actors) in extras.select_by_core.iter().enumerate() {
        out.push_str(&format!("        case {core}:\n"));
        if actors.is_empty() {
            out.push_str(&format!("            return {NO_EDGE}\n"));
        } else {
            out.push_str("            match slot:\n");
            for (slot, name) in actors.iter().enumerate() {
                let root = extras
                    .mailboxes
                    .iter()
                    .position(|m| m.name == *name)
                    .unwrap_or(NO_EDGE);
                out.push_str(&format!("                case {slot}:\n"));
                out.push_str(&format!("                    return {root}\n"));
            }
            out.push_str("                case _:\n");
            out.push_str(&format!("                    return {NO_EDGE}\n"));
        }
    }
    out.push_str("        case _:\n");
    out.push_str(&format!("            return {NO_EDGE}\n"));
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

    out.push_str(
        "pub fn __wrela_child_store_result(gi: usize, slot: usize, tag: u64, payload: u64) -> u64:\n",
    );
    out.push_str("    match slot:\n");
    for c in 0..max_children {
        out.push_str(&format!("        case {c}:\n"));
        out.push_str(&format!(
            "            GROUPS.slots[gi].child{c}_tag = tag\n"
        ));
        out.push_str(&format!(
            "            GROUPS.slots[gi].child{c}_payload = payload\n"
        ));
        out.push_str("            return 1\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

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

    emit_mb_u64_ladder(&mut out, "capacity", extras, |m| m.capacity);
    emit_mb_usize_ladder(&mut out, "slot_words", extras, |m| {
        (m.slot_size / 8) as usize
    });
    emit_mb_usize_ladder(&mut out, "turn_index", extras, |m| m.turn_index);
    emit_mb_usize_ladder(&mut out, "core", extras, |m| m.core);
    emit_mb_usize_ladder(&mut out, "state", extras, |m| m.state as usize);
    emit_mb_u64_ladder(&mut out, "has_lineage", extras, |m| {
        u64::from(m.frame_size >= crate::codegen::TURN_RECORD_SIZE + 16)
    });
    emit_mb_usize_ladder(&mut out, "method_count", extras, |m| m.methods.len());

    out.push_str("pub fn __wrela_mb_get_head(root: usize) -> u64:\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return MB{i}_CTL.head\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_set_head(root: usize, v: u64):\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            MB{i}_CTL.head = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_get_tail(root: usize) -> u64:\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return MB{i}_CTL.tail\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_set_tail(root: usize, v: u64):\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            MB{i}_CTL.tail = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_get_count(root: usize) -> u64:\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return MB{i}_CTL.count\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_set_count(root: usize, v: u64):\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            MB{i}_CTL.count = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_load_word(root: usize, wi: usize) -> u64:\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return MB{i}_DATA.words[wi]\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_mb_store_word(root: usize, wi: usize, v: u64):\n");
    out.push_str("    match root:\n");
    for i in 0..MB_POOL_COUNT {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            MB{i}_DATA.words[wi] = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_method_suspends(root: usize, method: usize) -> u64:\n");
    out.push_str("    match root:\n");
    for (ri, mb) in extras.mailboxes.iter().enumerate() {
        out.push_str(&format!("        case {ri}:\n"));
        if mb.methods.is_empty() {
            out.push_str("            return 0\n");
        } else {
            out.push_str("            match method:\n");
            for (mi, m) in mb.methods.iter().enumerate() {
                out.push_str(&format!("                case {mi}:\n"));
                out.push_str(&format!(
                    "                    return {}\n",
                    u64::from(m.is_async)
                ));
            }
            out.push_str("                case _:\n");
            out.push_str("                    return 0\n");
        }
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_method_is_aggregate(root: usize, method: usize) -> u64:\n");
    out.push_str("    match root:\n");
    for (ri, mb) in extras.mailboxes.iter().enumerate() {
        out.push_str(&format!("        case {ri}:\n"));
        if mb.methods.is_empty() {
            out.push_str("            return 0\n");
        } else {
            out.push_str("            match method:\n");
            for (mi, m) in mb.methods.iter().enumerate() {
                out.push_str(&format!("                case {mi}:\n"));
                out.push_str(&format!(
                    "                    return {}\n",
                    u64::from(m.reply_is_aggregate)
                ));
            }
            out.push_str("                case _:\n");
            out.push_str("                    return 0\n");
        }
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str(
        "pub fn __wrela_call_method(root: usize, method: usize, arg0: u64, arg1: u64, stage: u64) -> u64:\n",
    );
    out.push_str("    match root:\n");
    let mut flat = 0usize;
    for (ri, mb) in extras.mailboxes.iter().enumerate() {
        out.push_str(&format!("        case {ri}:\n"));
        if mb.methods.is_empty() {
            out.push_str("            return 0\n");
        } else {
            out.push_str("            match method:\n");
            for mi in 0..mb.methods.len() {
                out.push_str(&format!("                case {mi}:\n"));
                out.push_str(&format!(
                    "                    return __method_{flat}(arg0, arg1, stage)\n"
                ));
                flat += 1;
            }
            out.push_str("                case _:\n");
            out.push_str("                    return 0\n");
        }
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

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

    out.push_str("pub fn __wrela_enqueue_root(handle: usize) -> usize:\n");
    out.push_str("    match handle:\n");
    for (i, h) in extras.enqueue_handles.iter().enumerate() {
        out.push_str(&format!("        case {h}:\n"));
        out.push_str(&format!("            return {i}\n"));
    }
    out.push_str("        case _:\n");
    out.push_str(&format!("            return {NO_EDGE}\n"));
    out.push('\n');

    out.push_str("pub fn __wrela_init_nwords(slot: usize) -> usize:\n");
    out.push_str("    match slot:\n");
    for (i, &(_, nwords)) in init_spans.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {nwords}\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_init_store_word(slot: usize, wi: usize, v: u64):\n");
    out.push_str("    match slot:\n");
    for i in 0..init_spans.len() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            INIT_SPAN{i}.words[wi] = v\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_boot_call(i: usize):\n");
    out.push_str("    match i:\n");
    for i in 0..extras.n_boot_calls {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            __boot_call_{i}()\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_irq_mask(i: usize) -> u64:\n");
    out.push_str("    match i:\n");
    for (i, &bit) in extras.irq_vector_bits.iter().enumerate() {
        let mask = 1u64 << (bit & 63);
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {mask}\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');

    out.push_str("pub fn __wrela_irq_invoke(i: usize):\n");
    out.push_str("    match i:\n");
    for i in 0..extras.irq_vector_bits.len() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            __irq_call_{i}()\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    out.push_str("pub fn __wrela_wake_invoke(i: usize):\n");
    out.push_str("    match i:\n");
    for i in 0..extras.wake_pending_addrs.len() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            __wake_call_{i}()\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');

    assert!(
        extras.tests.len() <= TEST_CALL_POOL_COUNT,
        "image needs {} runtime tests; pool is {TEST_CALL_POOL_COUNT}",
        extras.tests.len()
    );
    push_const(&mut out, "N_TESTS", extras.tests.len());
    push_const(&mut out, "TEST_CALL_POOL_COUNT", TEST_CALL_POOL_COUNT);
    out.push_str(&format!(
        "pub const HAS_BOOT_INIT: bool = {}\n",
        if extras.has_boot_init {
            "true"
        } else {
            "false"
        }
    ));
    out.push('\n');
    for i in 0..TEST_CALL_POOL_COUNT {
        out.push_str(&format!("pub fn __test_call_{i}() -> u64:\n"));
        out.push_str("    return 0\n");
        out.push('\n');
        out.push_str(&format!("pub fn __test_prefix_{i}():\n"));
        out.push_str("    return\n");
        out.push('\n');
    }
    out.push_str("pub fn __wrela_test_call(i: usize) -> u64:\n");
    out.push_str("    match i:\n");
    for i in 0..extras.tests.len() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return __test_call_{i}()\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');
    out.push_str("pub fn __wrela_test_append_prefix(i: usize):\n");
    out.push_str("    match i:\n");
    for i in 0..extras.tests.len() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            __test_prefix_{i}()\n"));
        out.push_str("            return\n");
    }
    out.push_str("        case _:\n");
    out.push_str("            return\n");
    out.push('\n');
    out.push_str("pub fn __wrela_test_suspends(i: usize) -> u64:\n");
    out.push_str("    match i:\n");
    for (i, t) in extras.tests.iter().enumerate() {
        let v = if t.is_async { 1 } else { 0 };
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {v}\n"));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');
    out.push_str("pub fn __wrela_test_turn_index(i: usize) -> usize:\n");
    out.push_str("    match i:\n");
    for (i, t) in extras.tests.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", t.turn_index));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");

    Ok(out)
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

fn emit_mb_u64_ladder(
    out: &mut String,
    field: &str,
    extras: &RtconfigExtras,
    f: impl Fn(&MailboxFact) -> u64,
) {
    out.push_str(&format!("pub fn __wrela_mb_{field}(root: usize) -> u64:\n"));
    out.push_str("    match root:\n");
    for (i, m) in extras.mailboxes.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", f(m)));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');
}

fn emit_mb_usize_ladder(
    out: &mut String,
    field: &str,
    extras: &RtconfigExtras,
    f: impl Fn(&MailboxFact) -> usize,
) {
    out.push_str(&format!(
        "pub fn __wrela_mb_{field}(root: usize) -> usize:\n"
    ));
    out.push_str("    match root:\n");
    for (i, m) in extras.mailboxes.iter().enumerate() {
        out.push_str(&format!("        case {i}:\n"));
        out.push_str(&format!("            return {}\n", f(m)));
    }
    out.push_str("        case _:\n");
    out.push_str("            return 0\n");
    out.push('\n');
}

pub fn stub_call_remaps(extras: &RtconfigExtras, n_cores: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, site) in extras.child_sites.iter().enumerate() {
        out.push((format!("__resume_{i}"), site.callee_key.clone()));
    }
    if n_cores > 1 {
        out.push((
            "__wrela_secondary_entry_body".to_string(),
            "__wrela_rt_secondary_entry".to_string(),
        ));
    }
    out
}

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

pub fn parse_generated(text: &str) -> Result<Module, String> {
    let tokens = lexer::lex(text)
        .map_err(|e| format!("rtconfig lex: {} at {}:{}", e.message, e.line, e.col))?;
    parser::parse(tokens)
        .map_err(|e| format!("rtconfig parse: {} at {}:{}", e.message, e.line, e.col))
}

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

pub fn typecheck_batch2(generated_text: &str) -> Result<(), String> {
    if contains_forbidden_construct(generated_text) {
        return Err(
            "error[build]: generated rtconfig contains a forbidden construct \
             (while/for/async/@actor); facts-only rule"
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

pub fn generate_and_typecheck(tables: &RuntimeTables) -> Result<String, String> {
    let text = generate(tables)?;
    typecheck_batch2(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::RuntimeTables;

    fn sample_tables(cores: usize) -> RuntimeTables {
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
        let one = generate(&sample_tables(1)).unwrap();
        assert!(
            one.contains("pub const N_CORES: usize = 1\n"),
            "single-core must spell N_CORES exactly; got:\n{one}"
        );
        let two = generate(&sample_tables(2)).unwrap();
        assert!(
            two.contains("pub const N_CORES: usize = 2\n"),
            "cores=2 must spell N_CORES exactly; got:\n{two}"
        );
        let three = generate(&sample_tables(3)).unwrap();
        assert!(
            three.contains("pub const N_CORES: usize = 3\n"),
            "cross-core must spell N_CORES exactly; got:\n{three}"
        );
    }

    #[test]
    fn core_slots_matches_machine() {
        let text = generate(&sample_tables(1)).unwrap();
        let expected = format!(
            "pub const CORE_SLOTS: usize = {}\n",
            wrela_machine::CORE_SLOTS
        );
        assert!(
            text.contains(&expected),
            "rtconfig CORE_SLOTS must match wrela_machine::CORE_SLOTS; got:\n{text}"
        );
        assert_eq!(wrela_machine::CORE_SLOTS, 32);
    }

    #[test]
    fn secondary_entry_pool_is_exactly_n_cores_minus_one() {
        for n in [1usize, 2, 3] {
            let text = generate(&sample_tables(n)).unwrap();
            let expected = n.saturating_sub(1);
            let mut count = 0usize;
            for k in 1..=wrela_machine::CORE_SLOTS {
                if text.contains(&format!("pub fn __wrela_secondary_entry_{k}():\n")) {
                    count += 1;
                }
            }
            assert_eq!(
                count, expected,
                "N_CORES={n}: want {expected} secondary entries, found {count}:\n{text}"
            );
            if n > 1 {
                assert!(
                    text.contains("pub fn __wrela_secondary_entry_body(core: usize):\n"),
                    "N_CORES={n}: missing secondary body stub:\n{text}"
                );
            } else {
                assert!(
                    !text.contains("__wrela_secondary_entry_"),
                    "N_CORES=1 must emit no secondary trampoline pool:\n{text}"
                );
            }
            assert!(
                !text.contains(&format!("pub fn __wrela_secondary_entry_{n}():\n")),
                "N_CORES={n}: must not emit __wrela_secondary_entry_{n}:\n{text}"
            );
        }
    }

    #[test]
    fn placed_uses_rtdata_base_literal() {
        let text = generate(&sample_tables(1)).unwrap();
        assert!(!text.contains("@placed(RTDATA_BASE)"));
        assert!(
            text.contains("pub static RT: RuntimeTables\n"),
            "expected RT static; got:\n{text}"
        );
        assert!(
            !text.contains(&format!("@placed({RTDATA_BASE:#x})\npub static RT:")),
            "empty-turn RT must not claim RTDATA_BASE; got:\n{text}"
        );
        assert!(
            text.contains(&format!(
                "@placed({:#x})\npub static SCHED:",
                RTDATA_BASE + 128
            )),
            "expected empty-turn SCHED numeric place; got:\n{text}"
        );
        assert!(
            text.contains("pub static GROUPS: GroupArena\n"),
            "expected GROUPS static; got:\n{text}"
        );
    }

    #[test]
    fn facts_only_forbids_control_and_actors() {
        let text = generate(&sample_tables(1)).unwrap();
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
        let a = generate(&tables).unwrap();
        let b = generate(&tables).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn generated_text_parses() {
        let text = generate(&sample_tables(2)).unwrap();
        let module = parse_generated(&text).expect("parse");
        assert_eq!(module.path, vec!["__image_runtime".to_string()]);
    }

    #[test]
    fn structured_fields_present() {
        let mut t = sample_tables(1);
        t.group_arena_capacity = 1;
        t.total_bytes = 2048;
        let text = generate(&t).unwrap();
        assert!(text.contains("struct TurnArea:\n"));
        assert!(text.contains("struct GroupSlot:\n"));
        assert!(text.contains("struct SchedCore:\n"));
        assert!(text.contains("struct SchedStripe:\n"));
        assert!(text.contains("pub const GROUP_SLOTS: usize = 1\n"));
        assert!(text.contains("pub const GROUP_MAX_CHILDREN: usize = 2\n"));
        assert!(text.contains("pub const GROUP_SLOT_SIZE: usize = 96\n"));
        assert!(text.contains("turns: [TurnArea; N_TURNS_LEN]\n"));
        assert!(text.contains("cores: [SchedCore; N_CORES]\n"));
        assert!(text.contains("pub static SCHED: SchedStripe\n"));
        assert!(text.contains("slots: [GroupSlot; GROUP_SLOTS]\n"));
        assert!(text.contains("pub const N_CHILD_SITES: usize = 0\n"));
        assert!(text.contains("child0_tag: u64\n"));
        assert!(text.contains("child1_tag: u64\n"));
        assert!(text.contains("__wrela_child_store_result"));
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
        let text = generate(&t).unwrap();
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
