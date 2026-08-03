use std::collections::{BTreeMap, BTreeSet};

use crate::codegen::{CodegenProgram, Reloc};
use crate::encode;
use crate::eval::image::ImageGraph;
use crate::flowwir::{AwaitKind, FlowInst, FlowWirProgram, Transition};
use crate::mwir::{self, LayoutCtx};
use crate::sema::SemaError;
use crate::sema::typed::TypedProgram;
use crate::syntax::ast::Module;
#[cfg(test)]
use wrela_machine::console;
use wrela_machine::layout as machine_layout;

mod harness;
mod place;
mod report_lines;

mod boot_init;
mod rtdata;

pub(crate) use boot_init::device_index_of;
pub use rtdata::{
    ActorAddrs, ActorRuntimeLayout, DriverMailbox, DriverRuntimeLayout, GroupId, RingAddrs,
    RingKind, RingLayout, RuntimePlacement, RuntimeTables, TurnId, actor_method_index_tables,
    compute_runtime_tables, count_with_group_sites, mailbox_root_names, resolve_runtime_test_args,
    ring_data_stride_bytes, rings_padding_bytes, rings_reservation_bytes,
};
pub(crate) use rtdata::{
    MAILBOX_BOOKKEEPING_SIZE, REPLY_SLOT_SIZE, RR_CURSOR_SIZE, declared_mailbox_capacity,
    merge_actor_pub_methods, turn_owner,
};

pub use boot_init::BootCtx;
pub(crate) use boot_init::{
    BootInitArg, BootInitCall, RuntimeWiring, actor_inits, build_boot_init_calls,
    intern_fallible_init_abort_messages,
};

pub use place::place_runtime_tables;
pub use report_lines::{
    append_blk_vmm_lines, append_ring_vmm_lines, append_vmm_runtime_lines, attach_blk_report,
    parsed_runtime_tail, render_layout_section,
};

pub use harness::{
    DEADLOCK_MSG, EXIT_CODE_ABORT_FIXED, EXIT_CODE_ABORT_VAL, EXIT_CODE_NO_RUNTIME,
    TranscriptBound, build_checkpoint_and_vector_stub, build_checkpoint_and_vector_stub_ex,
    check_transcript_bound, compute_transcript_bound, lane1_pair_bytes, lane2_marker_bytes,
    lane2_pair_bytes, layout_test_image,
};

#[cfg(test)]
pub(crate) use harness::emitted_a64_census_live_counts;

use harness::{
    append_rodata, build_entry_stub, inject_boot_init_fn, inject_checkpoint_irq_fns,
    inject_rt_cross_core_fns, inject_rt_enqueue_and_dispatch_fns, push_halt, test_runner_facts,
};

#[cfg(test)]
use harness::Asm;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError {
    pub message: String,
}

impl LayoutError {
    pub(crate) fn new(message: impl Into<String>) -> LayoutError {
        LayoutError {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLayout {
    pub blob: Vec<u8>,
    pub linked: Option<crate::linked::LinkedProgram>,
    pub entry: u64,
    pub sections: Vec<Section>,
    pub runtime: Option<RuntimeTables>,
    pub pools: Vec<PoolPlacement>,
    pub device_regs: Vec<DeviceRegs>,
    pub blk: Option<BlkReport>,
    pub irq_host_injects: Vec<IrqHostInject>,
    pub core_entries: Vec<(usize, u64)>,
    pub cores: usize,
    pub placed_statics: Vec<PlacedStatic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedStatic {
    pub name: String,
    pub ty: String,
    pub addr: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkQueueReport {
    pub index: u16,
    pub size: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    pub doorbell: u64,
    pub pool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkReport {
    pub device: usize,
    pub capacity_sectors: u64,
    pub features: u64,
    pub vector: Option<u64>,
    pub queue: BlkQueueReport,
    pub descriptors_per_op: u16,
    pub occupancy_bound: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrqHostInject {
    pub base: u64,
    pub offset: u64,
    pub status: u32,
    pub vector: u64,
}

pub const IRQ_HOST_STATUS_MAGIC: u32 = 0x0000_A501;
pub const IRQ_STATUS_OFFSET: u64 = 0x60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolPlacement {
    pub backing: crate::eval::image_checks::PoolBacking,
    pub base: u64,
}

#[derive(Debug, Clone)]
pub struct IrqVectorEntry {
    pub vector: u64,
    pub handler_key: String,
    pub driver_state: u64,
}

#[derive(Debug, Clone)]
pub struct WakeDrainEntry {
    pub driver_state: u64,
    pub wake_drain_index: usize,
    pub task_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct GroupServiceCtx {
    pub arena_base: u64,
    pub arena_capacity: u64,
    pub turn_areas: Vec<(u64, TurnId)>,
}

pub struct CheckpointBlock {
    pub words: Vec<u32>,
    pub checkpoint_service_word: usize,
    pub deadline_poll_word: Option<usize>,
    pub has_deadline_poll: bool,
    pub relocs: Vec<Reloc>,
}

fn group_service_shape(runtime: Option<&RuntimeTables>) -> Option<GroupServiceCtx> {
    let tables = runtime.filter(|t| t.group_arena_capacity > 0)?;
    let n_driver_turns = tables
        .drivers
        .iter()
        .filter(|d| d.mailbox.is_some())
        .count();
    let n = tables.actors.len() + n_driver_turns + tables.free_turns.len();
    Some(GroupServiceCtx {
        arena_base: 0,
        arena_capacity: tables.group_arena_capacity,
        turn_areas: vec![(0, TurnId::from_index(0)); n],
    })
}

fn group_service_ctx(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
) -> Option<GroupServiceCtx> {
    if tables.group_arena_capacity == 0 {
        return None;
    }
    let mut turn_areas: Vec<(u64, TurnId)> = placement
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| (a.turn, TurnId::from_index(i)))
        .collect();
    let mut next_turn = tables.actors.len();
    for (i, d) in tables.drivers.iter().enumerate() {
        if d.mailbox.is_none() {
            continue;
        }
        let Some(addrs) = placement.driver_mailboxes.get(&i) else {
            continue;
        };
        turn_areas.push((addrs.turn, TurnId::from_index(next_turn)));
        next_turn += 1;
    }
    for (key, &addr) in &placement.free_turns {
        let Some(&id) = placement.turn_ids.get(key) else {
            continue;
        };
        turn_areas.push((addr, id));
    }
    Some(GroupServiceCtx {
        arena_base: placement.group_arena,
        arena_capacity: tables.group_arena_capacity,
        turn_areas,
    })
}

fn round_up(n: u64, align: u64) -> u64 {
    n.div_ceil(align) * align
}

fn steer_rtdata_base(cursor: u64, tables: &RuntimeTables) -> Result<u64, LayoutError> {
    if tables.total_bytes > machine_layout::RTDATA_SIZE_MAX {
        return Err(LayoutError::new(format!(
            "rtdata needs {} bytes (rings padding {}), which exceeds RTDATA_SIZE_MAX ({})",
            tables.total_bytes,
            tables.rings_padding,
            machine_layout::RTDATA_SIZE_MAX
        )));
    }
    let packed_end = round_up(cursor, 8);
    if packed_end > machine_layout::RTDATA_BASE {
        return Err(LayoutError::new(format!(
            "sections before rtdata end at {packed_end:#x}, past RTDATA_BASE ({:#x})",
            machine_layout::RTDATA_BASE
        )));
    }
    Ok(machine_layout::RTDATA_BASE)
}

fn pad_to(blob: &mut Vec<u8>, image_base: u64, target_addr: u64) {
    let want = (target_addr - image_base) as usize;
    debug_assert!(blob.len() <= want);
    blob.resize(want, 0);
}

const BL_HALF_RANGE_BYTES: i64 = 1i64 << 27;

const ADRP_HALF_RANGE_PAGES: i64 = 1i64 << 20;

const ADR_HALF_RANGE_BYTES: i64 = 1i64 << 20;

fn unresolved_call_target(target: &str, graph: Option<&ImageGraph>) -> LayoutError {
    let Some(actor) = crate::codegen::rt_enqueue_actor(target) else {
        return LayoutError::new(format!(
            "internal error: call target `{target}` was never codegen'd or registered as a \
             runtime-glue symbol"
        ));
    };
    let declared_driver = graph.is_some_and(|g| {
        g.drivers
            .iter()
            .any(|d| crate::sema::types::render_type(&d.actor_type) == actor)
    });
    if declared_driver {
        return LayoutError::new(format!(
            "this image sends to `{actor}` (an `await` or `send` through an `Actor[{actor}]` \
             handle), but `{actor}` is declared as a `@driver` with no `mailbox=` — a driver is \
             messageable only when its declaration says so (05-library.md §9): add \
             `mailbox=n` to `img.driver({actor}, ...)`, or remove the call. Without one there \
             is no mailbox to admit into and no admission routine to call"
        ));
    }
    LayoutError::new(format!(
        "this image sends to actor `{actor}` (an `await` or `send` through an \
         `Actor[{actor}]` handle) but never declares a `{actor}` instance — add \
         `img.actor({actor}, mailbox=...)` to the `@image` fn, or remove the call: a \
         handle type with no declared instance has no mailbox to admit into"
    ))
}

fn xsend_trampoline(edge: usize) -> String {
    format!("__wrela_xsend_{edge}")
}

fn xreply_trampoline(edge: usize) -> String {
    format!("__wrela_xreply_{edge}")
}

fn caller_core(caller_key: &str, w: &RuntimeWiring) -> usize {
    attributed_core(caller_key, w).unwrap_or(0)
}

fn attributed_core(caller_key: &str, w: &RuntimeWiring) -> Option<usize> {
    let actor_names: Vec<String> = w.tables.actors.iter().map(|a| a.name.clone()).collect();
    let driver_names: Vec<String> = w.tables.drivers.iter().map(|d| d.name.clone()).collect();
    if let Some(owner) =
        turn_owner(caller_key, &actor_names).or_else(|| turn_owner(caller_key, &driver_names))
    {
        return Some(w.placement.core_of_actor_type(owner).unwrap_or(0));
    }
    if w.tables.free_turns.iter().any(|(k, _)| k == caller_key) {
        return Some(0);
    }
    None
}

fn resolve_cross_core_edge(
    caller_key: &str,
    target: &str,
    wiring: Option<&RuntimeWiring>,
) -> Result<Option<String>, LayoutError> {
    let Some(w) = wiring else {
        return Ok(None);
    };
    if w.placement.cores <= 1 {
        return Ok(None);
    }
    if crate::codegen::is_compiler_glue_symbol(caller_key) {
        return Ok(None);
    }
    let Some(target_actor) = crate::codegen::rt_enqueue_actor(target) else {
        return Ok(None);
    };
    let caller = caller_core(caller_key, w);
    let Some(target_core) = w.placement.core_of_actor_type(&target_actor) else {
        return Err(LayoutError::new(format!(
            "this image declares `{target_actor}` instances on more than one core, but the \
             generated admission routine (`{target}`) is per actor struct, not per instance — \
             give each instance its own struct, or place them on one core (plans/M8.md item C1)"
        )));
    };
    if caller == target_core {
        return Ok(None);
    }
    let edge = w.tables.rings.iter().enumerate().find_map(|(i, r)| {
        if r.kind == RingKind::Request
            && r.src == caller
            && r.actor.as_deref() == Some(target_actor)
        {
            Some(i)
        } else {
            None
        }
    });
    match edge {
        Some(edge) => {
            if edge >= crate::rtconfig::RING_POOL_COUNT {
                return Err(LayoutError::new(format!(
                    "image needs request-ring edge {edge}; trampoline pool is {} (plans/M11.md decision 802)",
                    crate::rtconfig::RING_POOL_COUNT
                )));
            }
            Ok(Some(xsend_trampoline(edge)))
        }
        None => {
            if w.tables.rings.is_empty() {
                Ok(Some(format!(
                    "__wrela_xsend_pending {caller} {target_actor}"
                )))
            } else {
                Err(LayoutError::new(format!(
                    "internal error: cross-core edge {caller} -> `{target_actor}` has no request ring"
                )))
            }
        }
    }
}

fn resolve_xreply_edge(target: &str, w: &RuntimeWiring) -> Option<String> {
    let (src, dst) = crate::codegen::rt_xreply_cores(target)?;
    let edge = w.tables.rings.iter().enumerate().find_map(|(i, r)| {
        if r.kind == RingKind::Reply && r.src == src && r.dst == dst {
            Some(i)
        } else {
            None
        }
    })?;
    if edge >= crate::rtconfig::RING_POOL_COUNT {
        return None;
    }
    Some(xreply_trampoline(edge))
}

fn cross_core_edges(
    flow: &FlowWirProgram,
    w: &RuntimeWiring,
) -> Result<BTreeSet<(usize, String)>, LayoutError> {
    let mut out = BTreeSet::new();
    if w.placement.cores <= 1 {
        return Ok(out);
    }
    for (key, f) in &flow.fns {
        let mut method_keys: Vec<String> = Vec::new();
        for state in &f.states {
            for op in &state.ops {
                if let FlowInst::Send { method_key, .. } = op {
                    method_keys.push(method_key.clone());
                }
            }
            if let Transition::Await {
                what: AwaitKind::ActorCall { method_key, .. },
                ..
            } = &state.transition
            {
                method_keys.push(method_key.clone());
            }
        }
        for mk in method_keys {
            let actor = mk.split('.').next().unwrap_or(mk.as_str()).to_string();
            let target = crate::codegen::rt_enqueue_symbol(&actor);
            if let Some(sym) = resolve_cross_core_edge(key, &target, Some(w))? {
                debug_assert!(
                    sym.starts_with("__wrela_xsend_"),
                    "cross-core redirect must be an xsend trampoline, got {sym}"
                );
                out.insert((caller_core(key, w), actor));
            }
        }
    }
    Ok(out)
}

fn cross_core_rings(
    flow: &FlowWirProgram,
    w: &RuntimeWiring,
) -> Result<Vec<RingLayout>, LayoutError> {
    let edges = cross_core_edges(flow, w)?;
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    let mut requests: Vec<RingLayout> = Vec::new();
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (src, actor) in &edges {
        let Some(dst) = w.placement.core_of_actor_type(actor) else {
            return Err(LayoutError::new(format!(
                "internal error: cross-core edge to `{actor}` has no single placed core"
            )));
        };
        let Some((mailbox_capacity, slot_size)) = mailbox_root_shape(&w.tables, actor) else {
            return Err(LayoutError::new(format!(
                "internal error: cross-core edge to `{actor}`, which has no runtime mailbox"
            )));
        };
        if mailbox_capacity == 0 {
            return Err(LayoutError::new(format!(
                "core {src} sends to actor `{actor}` on core {dst}, but `{actor}` declares a \
                 mailbox capacity of 0: a cross-core edge's ring is sized from the mailbox it \
                 feeds (04-compiler.md §3, plans/M8.md item C2), so there is no capacity to \
                 derive — declare `mailbox=` on that `img.actor(...)`"
            )));
        }
        requests.push(RingLayout {
            src: *src,
            dst,
            kind: RingKind::Request,
            actor: Some(actor.to_string()),
            capacity: mailbox_capacity,
            slot_size,
        });
        pairs.insert((*src, dst));
    }
    let mut replies: Vec<RingLayout> = Vec::new();
    for (src, dst) in &pairs {
        let capacity = reply_ring_capacity(w, *src);
        if capacity == 0 {
            return Err(LayoutError::new(format!(
                "core {src} sends to core {dst}, but core {src} owns no turn area, so the reply \
                 ring's capacity cannot be derived (plans/M8.md item C2)"
            )));
        }
        replies.push(RingLayout {
            src: *dst,
            dst: *src,
            kind: RingKind::Reply,
            actor: None,
            capacity,
            slot_size: REPLY_SLOT_SIZE,
        });
    }
    requests.sort_by(|a, b| (a.src, a.dst, &a.actor).cmp(&(b.src, b.dst, &b.actor)));
    replies.sort_by(|a, b| (a.src, a.dst).cmp(&(b.src, b.dst)));
    requests.extend(replies);
    Ok(requests)
}

fn reject_unlowerable_cross_core_shapes(
    rings: &[RingLayout],
    w: &RuntimeWiring,
    boot: &BootCtx,
    flow: &FlowWirProgram,
) -> Result<(), LayoutError> {
    let _ = boot;
    if w.placement.cores <= 1 {
        return Ok(());
    }
    for ring in rings.iter().filter(|r| r.kind == RingKind::Request) {
        let Some(actor) = &ring.actor else { continue };
        let Some((_, methods)) = w.dispatch.iter().find(|(name, _)| name == actor) else {
            continue;
        };
        if let Some((key, _, _)) = methods.iter().find(|(_, _, agg)| *agg) {
            return Err(LayoutError::new(format!(
                "core {} sends to actor `{actor}` on core {}, but `{key}` declares an aggregate \
                 reply: an aggregate is written straight into the awaiting turn's frame through \
                 `x8`, which does not travel the cross-core reply ring this edge lowers to \
                 (04-compiler.md §3, plans/M8.md item C2). Place both ends on one core, or make \
                 the reply a scalar",
                ring.src, ring.dst
            )));
        }
    }
    for (key, f) in &flow.fns {
        let has_checkpoint = f
            .states
            .iter()
            .enumerate()
            .any(|(i, s)| match &s.transition {
                Transition::Jump(t) if *t <= i => true,
                Transition::Branch {
                    then_state,
                    else_state,
                    ..
                } if *then_state <= i || *else_state <= i => true,
                _ => false,
            });
        if !has_checkpoint {
            continue;
        }
        match attributed_core(key, w) {
            Some(0) => {}
            Some(core) => {
                return Err(LayoutError::new(format!(
                    "`{key}` runs on core {core} and contains a checkpoint (a loop back-edge), \
                     but `__wrela_checkpoint_service` and every checkpoint test name core 0's \
                     own pending word by construction — servicing one from core {core} would \
                     clear the wake a cross-core ring raised for core 0. Place this actor on \
                     core 0, or remove the loop; per-core checkpoint services are not part of \
                     plans/M8.md item C2"
                )));
            }
            None => {
                return Err(LayoutError::new(format!(
                    "`{key}` contains a checkpoint (a loop back-edge) and is not owned by any \
                     declared actor or driver, so this build cannot prove which core it runs \
                     on. In a multi-core image that is refused: a checkpoint serviced from a \
                     secondary core clears core 0's pending word and eats the wake a cross-core \
                     ring raised for it. Move the loop into a method of an actor placed on core \
                     0, or remove it; per-core checkpoint services are not part of plans/M8.md \
                     item C2"
                )));
            }
        }
    }
    Ok(())
}

pub fn mailbox_enqueue_specs(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<Vec<(String, u64, u64)>, String> {
    if graph.actors.is_empty() && graph.drivers.is_empty() {
        return Ok(Vec::new());
    }
    let shapes = merge_actor_pub_methods(modules, layout_ctx).map_err(|e| e.message)?;
    let mut out = Vec::new();
    for decl in &graph.actors {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let mailbox_capacity = declared_mailbox_capacity(&decl.args, &format!("actor `{name}`"))?
            .ok_or_else(|| {
            format!(
                "actor `{name}` has no declared `mailbox=` capacity (plans/M6.md decision 3: \
                 the declared bound is the whole of M6's own mailbox-capacity story; derivation \
                 is out of scope)"
            )
        })?;
        let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
        let max_args_bytes = methods
            .iter()
            .map(|m| m.param_sizes.iter().sum::<u64>())
            .max()
            .unwrap_or(0);
        let slot_size = 16 + max_args_bytes;
        out.push((name, mailbox_capacity, slot_size));
    }
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let Some(capacity) = declared_mailbox_capacity(&decl.args, &format!("driver `{name}`"))?
        else {
            continue;
        };
        let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
        let max_args_bytes = methods
            .iter()
            .map(|m| m.param_sizes.iter().sum::<u64>())
            .max()
            .unwrap_or(0);
        let slot_size = 16 + max_args_bytes;
        out.push((name, capacity, slot_size));
    }
    Ok(out)
}

fn resolve_mailbox_actor_addrs(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
    name: &str,
) -> Option<ActorAddrs> {
    if let Some((i, _)) = tables
        .actors
        .iter()
        .enumerate()
        .find(|(_, a)| a.name == name)
    {
        return placement.actors.get(i).copied();
    }
    for (i, d) in tables.drivers.iter().enumerate() {
        if d.mailbox.is_some() && d.name == name {
            return placement.driver_mailboxes.get(&i).copied();
        }
    }
    None
}

fn mailbox_root_shape(tables: &RuntimeTables, name: &str) -> Option<(u64, u64)> {
    if let Some(a) = tables.actors.iter().find(|a| a.name == name) {
        return Some((a.mailbox_capacity, a.slot_size));
    }
    tables
        .drivers
        .iter()
        .find(|d| d.name == name)
        .and_then(|d| d.mailbox.as_ref())
        .map(|m| (m.capacity, m.slot_size))
}

fn reply_ring_capacity(w: &RuntimeWiring, core: usize) -> u64 {
    let actors = w.actor_cores.iter().filter(|c| **c == core).count() as u64;
    let free = if core == 0 {
        w.tables.free_turns.len() as u64
    } else {
        0
    };
    actors + free
}

fn patch_bl(
    words: &mut [u32],
    idx: usize,
    this_addr: u64,
    target_addr: u64,
) -> Result<(), LayoutError> {
    let delta = target_addr as i64 - this_addr as i64;
    if delta <= -BL_HALF_RANGE_BYTES || delta >= BL_HALF_RANGE_BYTES {
        return Err(LayoutError::new(format!(
            "relocation out of range: a `BL` at {this_addr:#x} targets {target_addr:#x} \
             ({delta} bytes away) — outside the imm26 encoder's own +/-128 MiB reach"
        )));
    }
    if (words[idx] >> 26) & 0b1_1111 != 0b00101 {
        return Err(LayoutError::new(format!(
            "internal error: a call relocation names word {idx} at {this_addr:#x}, which holds              {:#010x} — not a `B`/`BL`",
            words[idx]
        )));
    }
    let links = words[idx] & 0x8000_0000 != 0;
    words[idx] = if links {
        encode::enc_bl(delta as i32)
    } else {
        encode::enc_b(delta as i32)
    };
    Ok(())
}

fn verify_conventions_after_layout(program: &CodegenProgram) -> Result<(), LayoutError> {
    crate::codegen::verify_conventions(program).map_err(|e| {
        LayoutError::new(format!(
            "{e}.\n\nThis check runs *after* layout's `inject_*` and floor substitutions, so \
             the usual cause is a body this stage replaced or aliased under a key codegen had \
             already published a convention for. A key a later stage may own must be opaque to \
             the whole-program allocator (`regalloc::FnInput::opaque_body`), never given a \
             measured clobber set."
        ))
    })
}

fn expand_rodata_adr_sites(
    program: &CodegenProgram,
    sites: &BTreeMap<String, BTreeSet<usize>>,
) -> Result<CodegenProgram, LayoutError> {
    let mut out = program.clone();
    for (key, f) in &program.fns {
        let Some(site_words) = sites.get(key) else {
            continue;
        };
        let mut code = Vec::with_capacity(f.code.len() + site_words.len());
        let mut old_to_new = vec![usize::MAX; f.code.len() + 1];
        for (i, word) in f.code.iter().enumerate() {
            old_to_new[i] = code.len();
            if !site_words.contains(&i) {
                code.push(word.clone());
                continue;
            }
            if word.rule != crate::cost::CostRule::Adrp
                || !(word.word & 0x9f00_0000 == 0x1000_0000
                    || word.word & 0x9f00_0000 == 0x9000_0000)
            {
                return Err(LayoutError::new(format!(
                    "cannot grow `RodataAdr` at `{key}[{i}]`: the site is not an ADR/ADRP word"
                )));
            }
            let reg = (word.word & 0x1f) as u8;
            let mut adrp = word.clone();
            adrp.word = encode::enc_adrp(reg, 0);
            adrp.text = adrp.text.replacen("adr ", "adrp ", 1);
            code.push(adrp);
            code.push(crate::cost::EmittedWord::new(
                encode::enc_add_imm(reg, reg, 0, true),
                format!("add x{reg}, x{reg}, #0"),
                crate::cost::CostRule::Alu,
                Some(reg),
                &[reg],
            ));
        }
        old_to_new[f.code.len()] = code.len();
        let mut relocs = Vec::with_capacity(f.relocs.len());
        for reloc in &f.relocs {
            let old = crate::relax::reloc_word(reloc);
            let Some(&new) = old_to_new.get(old) else {
                return Err(LayoutError::new(format!(
                    "relocation at `{key}[{old}]` is outside its function"
                )));
            };
            if let Reloc::RodataAdr { byte_offset, .. } = reloc {
                if site_words.contains(&old) {
                    relocs.push(Reloc::Rodata {
                        word_adrp: new,
                        byte_offset: *byte_offset,
                    });
                    continue;
                }
            }
            relocs.push(crate::relax::remap_reloc(reloc, new));
        }
        let output = out.fns.get_mut(key).ok_or_else(|| {
            LayoutError::new(format!("missing function `{key}` while growing ADR"))
        })?;
        output.code = code;
        output.relocs = relocs;
    }
    Ok(out)
}

fn patch_load_imm_words(words: &mut [u32], word: usize, value: u64) {
    let rd = (words[word] & 0x1F) as u8;
    words[word] = encode::enc_movz(rd, (value & 0xFFFF) as u16, 0, true);
    words[word + 1] = encode::enc_movk(rd, ((value >> 16) & 0xFFFF) as u16, 16, true);
    words[word + 2] = encode::enc_movk(rd, ((value >> 32) & 0xFFFF) as u16, 32, true);
    words[word + 3] = encode::enc_movk(rd, ((value >> 48) & 0xFFFF) as u16, 48, true);
}

fn driver_declares_task(modules: &BTreeMap<String, Module>, name: &str) -> bool {
    !driver_task_method_names(modules, name).is_empty()
}

fn driver_task_method_names(modules: &BTreeMap<String, Module>, name: &str) -> Vec<String> {
    let bare = name.split('[').next().unwrap_or(name);
    let mut out = Vec::new();
    for m in modules.values() {
        for item in &m.items {
            let crate::syntax::ast::Item::Struct(s) = item else {
                continue;
            };
            if s.name != bare {
                continue;
            }
            if !s.attrs.iter().any(|a| a.name == "driver") {
                continue;
            }
            for mem in &s.members {
                if let crate::syntax::ast::Member::Fn(f) = mem {
                    if f.attrs.iter().any(|a| a.name == "task") {
                        out.push(f.name.clone());
                    }
                }
            }
        }
    }
    out
}

fn checkpoint_irq_shape(
    boot: Option<&BootCtx>,
    placement: Option<&RuntimePlacement>,
    tables: Option<&RuntimeTables>,
) -> (Vec<IrqVectorEntry>, Vec<WakeDrainEntry>) {
    let Some(boot) = boot else {
        return (Vec::new(), Vec::new());
    };
    let mut irq_vectors = Vec::new();
    let mut wake_drains = Vec::new();
    for (di, decl) in boot.graph.drivers.iter().enumerate() {
        let crate::sema::types::Type::Named(driver, targs) = &decl.actor_type else {
            continue;
        };
        let state = placement
            .and_then(|p| p.drivers.get(di).copied())
            .unwrap_or(0);
        let key_prefix = if targs.is_empty() {
            driver.clone()
        } else {
            format!(
                "struct:{}",
                crate::sema::types::render_type(&decl.actor_type)
            )
        };
        let vector = device_index_of(&decl.args)
            .and_then(|i| boot.graph.devices.get(i))
            .and_then(|d| crate::eval::image_checks::device_vector(&d.args));
        if let Some(v) = vector {
            for handler in irq_bind_handlers_in_driver(boot.modules, driver) {
                irq_vectors.push(IrqVectorEntry {
                    vector: v,
                    handler_key: format!("{key_prefix}.{handler}"),
                    driver_state: state,
                });
            }
        }
        if let Some(tables) = tables {
            if tables.drivers.get(di).is_some_and(|d| d.has_wake) {
                for task in driver_task_method_names(boot.modules, driver) {
                    let wake_drain_index = wake_drains.len();
                    wake_drains.push(WakeDrainEntry {
                        driver_state: state,
                        wake_drain_index,
                        task_key: format!("{key_prefix}.{task}"),
                    });
                }
            }
        }
    }
    (irq_vectors, wake_drains)
}

fn irq_bind_handlers_in_driver(modules: &BTreeMap<String, Module>, driver: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in modules.values() {
        for item in &m.items {
            let crate::syntax::ast::Item::Struct(s) = item else {
                continue;
            };
            if s.name != driver {
                continue;
            }
            for mem in &s.members {
                let body: &[crate::syntax::ast::Stmt] = match mem {
                    crate::syntax::ast::Member::Fn(f) => f.body.as_deref().unwrap_or(&[]),
                    crate::syntax::ast::Member::Init(i) => &i.body,
                    _ => continue,
                };
                collect_bind_handlers_stmts(body, &mut out);
            }
        }
    }
    out
}

fn collect_bind_handlers_stmts(stmts: &[crate::syntax::ast::Stmt], out: &mut Vec<String>) {
    use crate::syntax::ast::Stmt;
    for s in stmts {
        match s {
            Stmt::Expr(_, e) | Stmt::Send(_, e) => collect_bind_handlers_expr(e, out),
            Stmt::Assign(a) => {
                collect_bind_handlers_expr(&a.target, out);
                collect_bind_handlers_expr(&a.value, out);
            }
            Stmt::Return(_, Some(e)) => collect_bind_handlers_expr(e, out),
            Stmt::If(i) => {
                collect_bind_handlers_expr(&i.cond, out);
                collect_bind_handlers_stmts(&i.then_branch, out);
                for elif in &i.elifs {
                    collect_bind_handlers_expr(&elif.cond, out);
                    collect_bind_handlers_stmts(&elif.body, out);
                }
                if let Some(b) = &i.else_branch {
                    collect_bind_handlers_stmts(b, out);
                }
            }
            Stmt::Match(m) => {
                collect_bind_handlers_expr(&m.scrutinee, out);
                for arm in &m.arms {
                    collect_bind_handlers_stmts(&arm.body, out);
                }
            }
            Stmt::While(w) => {
                collect_bind_handlers_expr(&w.cond, out);
                collect_bind_handlers_stmts(&w.body, out);
            }
            Stmt::For(f) => {
                collect_bind_handlers_expr(&f.iterable, out);
                collect_bind_handlers_stmts(&f.body, out);
            }
            Stmt::ComptimeIf(c) => {
                collect_bind_handlers_stmts(&c.then_branch, out);
                if let Some(b) = &c.else_branch {
                    collect_bind_handlers_stmts(b, out);
                }
            }
            Stmt::With(w) => {
                collect_bind_handlers_expr(&w.expr, out);
                collect_bind_handlers_stmts(&w.body, out);
            }
            Stmt::Defer(d) => match &d.body {
                crate::syntax::ast::DeferBody::Suite(body) => {
                    collect_bind_handlers_stmts(body, out);
                }
                crate::syntax::ast::DeferBody::Expr(e) => collect_bind_handlers_expr(e, out),
            },
            Stmt::Assert(a) => {
                collect_bind_handlers_expr(&a.cond, out);
                if let Some(m) = &a.message {
                    collect_bind_handlers_expr(m, out);
                }
            }
            Stmt::ComptimeAssert(_, cond, msg) => {
                collect_bind_handlers_expr(cond, out);
                if let Some(m) = msg {
                    collect_bind_handlers_expr(m, out);
                }
            }
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Return(_, None)
            | Stmt::Pass(_)
            | Stmt::Dmb(_) => {}
        }
    }
}

fn collect_bind_handlers_expr(e: &crate::syntax::ast::Expr, out: &mut Vec<String>) {
    use crate::syntax::ast::Expr;
    match e {
        Expr::Call(callee, _, args) => {
            if let Expr::Field(_, _, method) = callee.as_ref() {
                if method == "bind" {
                    for a in args {
                        if let Expr::Field(base, _, handler) = &a.value {
                            if matches!(base.as_ref(), Expr::Name(_, n) if n == "self") {
                                out.push(handler.clone());
                            }
                        }
                    }
                }
            }
            collect_bind_handlers_expr(callee, out);
            for a in args {
                collect_bind_handlers_expr(&a.value, out);
            }
        }
        Expr::Field(base, _, _)
        | Expr::Unary(_, _, base)
        | Expr::Not(_, base)
        | Expr::Try(_, base)
        | Expr::Send(_, base) => collect_bind_handlers_expr(base, out),
        Expr::Index(base, _, args) => {
            collect_bind_handlers_expr(base, out);
            for a in args {
                collect_bind_handlers_expr(a, out);
            }
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            collect_bind_handlers_expr(l, out);
            collect_bind_handlers_expr(r, out);
        }
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                collect_bind_handlers_expr(i, out);
            }
        }
        Expr::ArrayRepeat(_, elem, count) => {
            collect_bind_handlers_expr(elem, out);
            collect_bind_handlers_expr(count, out);
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                collect_bind_handlers_expr(&a.value, out);
            }
        }
        Expr::Range(_, a, b, _) => {
            collect_bind_handlers_expr(a, out);
            collect_bind_handlers_expr(b, out);
        }
        Expr::Is(_, scrutinee, _) => collect_bind_handlers_expr(scrutinee, out),
        Expr::Closure(c) => {
            if let crate::syntax::ast::ClosureBody::Expr(e) = &c.body {
                collect_bind_handlers_expr(e, out);
            }
        }
        Expr::Name(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(..)
        | Expr::FStr(_) => {}
    }
}

fn build_irq_host_injects(
    boot: Option<&BootCtx>,
    device_regs: &[DeviceRegs],
) -> Vec<IrqHostInject> {
    let Some(boot) = boot else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in device_regs {
        let Some(dev) = boot.graph.devices.get(r.device) else {
            continue;
        };
        let Some(vector) = crate::eval::image_checks::device_vector(&dev.args) else {
            continue;
        };
        let bare = r.driver.split('[').next().unwrap_or(r.driver.as_str());
        if irq_bind_handlers_in_driver(boot.modules, bare).is_empty() {
            continue;
        }
        out.push(IrqHostInject {
            base: r.base,
            offset: IRQ_STATUS_OFFSET,
            status: IRQ_HOST_STATUS_MAGIC,
            vector,
        });
    }
    out
}

fn driver_wake_pending_addr(
    _placement: &RuntimePlacement,
    tables: &RuntimeTables,
    driver: &str,
) -> Result<u64, LayoutError> {
    for d in &tables.drivers {
        let bare = d.name.split('[').next().unwrap_or(d.name.as_str());
        if d.name != driver && bare != driver {
            continue;
        }
        let Some(idx) = d.wake_drain_index else {
            return Err(LayoutError::new(format!(
                "internal error: `Wake` for `{driver}` but that driver has no `@task` \
                 (no wake-pending drain was reserved)"
            )));
        };
        let Some(&addr) = tables.wake_pending_addrs.get(idx) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` wake drain {idx} has no WAKE address"
            )));
        };
        return Ok(addr);
    }
    Err(wake_driver_undeclared(driver))
}

fn driver_state_addr(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
    driver: &str,
) -> Result<u64, LayoutError> {
    for (i, d) in tables.drivers.iter().enumerate() {
        let bare = d.name.split('[').next().unwrap_or(d.name.as_str());
        if d.name != driver && bare != driver {
            continue;
        }
        let Some(&state_base) = placement.drivers.get(i) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has no placed state"
            )));
        };
        return Ok(state_base);
    }
    Err(LayoutError::new(format!(
        "internal error: Reloc::DriverState names `@driver` `{driver}`, which this image's \
         runtime tables never placed"
    )))
}

fn wake_driver_undeclared(driver: &str) -> LayoutError {
    LayoutError::new(format!(
        "`wake` names `@driver` `{driver}`, which this image never declared — add \
         `img.driver({driver}, ...)` to the `@image` fn, or remove the `wake` \
         (03-hardware.md §6)"
    ))
}

fn turns_deref_needs_rtdata() -> String {
    "internal error: a virtqueue drain needs the `RT.turns` base and stride to reach the turn a \
     slot's waiter/reply-stage names, but this image's runtime tables were never placed"
        .to_string()
}

fn wake_needs_rtdata(driver: &str) -> LayoutError {
    LayoutError::new(format!(
        "`wake` for `@driver` `{driver}` needs a sealed `@image` that declares that driver — \
         this layout has no runtime tables for it. Add `img.driver({driver}, ...)` to an \
         `@image` fn, or remove the `wake` (03-hardware.md §6)"
    ))
}

fn driver_irq_vector(graph: Option<&ImageGraph>, driver: &str) -> Result<u64, LayoutError> {
    let bare_want = driver
        .strip_prefix("struct:")
        .unwrap_or(driver)
        .split('[')
        .next()
        .unwrap_or(driver);
    let Some(graph) = graph else {
        return Err(irq_driver_undeclared(bare_want));
    };
    for decl in &graph.drivers {
        let crate::sema::types::Type::Named(name, _) = &decl.actor_type else {
            continue;
        };
        if name != driver && name != bare_want {
            continue;
        }
        let Some(i) = device_index_of(&decl.args) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has a `LoadIrqVector` but no `device=` \
                 binding"
            )));
        };
        let Some(dev) = graph.devices.get(i) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` binds device#{i}, which does not exist"
            )));
        };
        return crate::eval::image_checks::device_vector(&dev.args).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has a `LoadIrqVector` but its device \
                 declared no `vector=` — `check_vector_bindings` should have rejected first"
            ))
        });
    }
    Err(irq_driver_undeclared(bare_want))
}

fn irq_driver_undeclared(driver: &str) -> LayoutError {
    LayoutError::new(format!(
        "`LoadIrqVector` names `@driver` `{driver}`, which this image never declared — add \
         `img.driver({driver}, device=...)` with `vector=N` (1..=63) on the device to an \
         `@image` fn, or drop the IRQ bind for a poll build (03-hardware.md §6/§7)"
    ))
}

fn patch_adrp_add(
    words: &mut [u32],
    word_adrp: usize,
    this_addr: u64,
    target_addr: u64,
) -> Result<(), LayoutError> {
    let reg = (words[word_adrp] & 0x1F) as u8;
    let this_page = this_addr & !0xFFF;
    let target_page = target_addr & !0xFFF;
    let page_delta = (target_page as i64 - this_page as i64) / 4096;
    if page_delta < -ADRP_HALF_RANGE_PAGES || page_delta >= ADRP_HALF_RANGE_PAGES {
        return Err(LayoutError::new(format!(
            "relocation out of range: an `ADRP` at {this_addr:#x} targets page {target_page:#x} \
             ({page_delta} pages away) — outside the imm21 encoder's own range"
        )));
    }
    words[word_adrp] = encode::enc_adrp(reg, page_delta as i32);
    words[word_adrp + 1] = encode::enc_add_imm(reg, reg, (target_addr & 0xFFF) as u16, true);
    Ok(())
}

fn patch_adr(
    words: &mut [u32],
    word_adr: usize,
    this_addr: u64,
    target_addr: u64,
) -> Result<(), LayoutError> {
    let reg = (words[word_adr] & 0x1F) as u8;
    let delta = target_addr as i64 - this_addr as i64;
    if !(-ADR_HALF_RANGE_BYTES..ADR_HALF_RANGE_BYTES).contains(&delta) {
        return Err(adr_out_of_range(this_addr, target_addr, delta));
    }
    words[word_adr] = encode::enc_adr(reg, delta as i32);
    Ok(())
}

fn adr_out_of_range(this_addr: u64, target_addr: u64, delta: i64) -> LayoutError {
    LayoutError::new(format!(
        "relocation out of range: an `ADR` at {this_addr:#x} targets {target_addr:#x}, \
         {delta} bytes away — outside `ADR`'s own ±1 MiB (±{ADR_HALF_RANGE_BYTES} byte) \
         reach. `OptId::AdrAddressing` (plans/codegen-pareto.md item B) substitutes one \
         `ADR` for an `ADRP`+`ADD` pair only where the whole image proves every site is \
         in reach; this image is too large between its code and its rodata for that. \
         Build in `dev`, or drop `OptId::AdrAddressing` from `RELEASE_OPTS`, to get the \
         two-word page-relative form back."
    ))
}

fn verify_section_sizes(
    sections: &[Section],
    image_base: u64,
    blob_len: u64,
) -> Result<(), LayoutError> {
    let Some(first) = sections.first() else {
        return Err(LayoutError::new(
            "internal error: an image with no sections at all",
        ));
    };
    if first.base != image_base {
        return Err(LayoutError::new(format!(
            "internal error: the first section `{}` does not start at IMAGE_BASE ({:#x} != {:#x})",
            first.name, first.base, image_base
        )));
    }
    for pair in sections.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let a_end = a.base + a.size;
        if a_end > b.base {
            return Err(LayoutError::new(format!(
                "internal error: section `{}` (ends {:#x}) overlaps section `{}` (starts {:#x})",
                a.name, a_end, b.name, b.base
            )));
        }
        let gap = b.base - a_end;
        let steered_rtdata = b.name == "rtdata"
            && b.base == machine_layout::RTDATA_BASE
            && a_end <= machine_layout::RTDATA_BASE;
        if gap >= 8 && !steered_rtdata {
            return Err(LayoutError::new(format!(
                "internal error: a {gap}-byte gap between section `{}` and `{}` exceeds every                  alignment this module ever rounds to",
                a.name, b.name
            )));
        }
    }

    let last = sections.last().expect("checked non-empty above");
    let want_len = last.base + last.size - image_base;
    if blob_len != want_len {
        return Err(LayoutError::new(format!(
            "internal error: the emitted blob is {blob_len} bytes but the section table implies {want_len}"
        )));
    }
    verify_branch_region(sections)?;
    Ok(())
}

pub const REGION_BYTES: u64 = 2 * 1024 * 1024;

pub fn same_region_holds(lo: u64, hi: u64) -> bool {
    if hi <= lo {
        return true;
    }
    lo / REGION_BYTES == (hi - 1) / REGION_BYTES
}

fn verify_branch_region(sections: &[Section]) -> Result<(), LayoutError> {
    let branchable: Vec<&Section> = sections
        .iter()
        .filter(|s| matches!(s.name, "entry" | "code" | "abort" | "checkpoint"))
        .collect();
    let (Some(first), Some(last)) = (branchable.first(), branchable.last()) else {
        return Ok(());
    };
    let lo = first.base;
    let hi = last.base + last.size;
    if !same_region_holds(lo, hi) {
        return Err(LayoutError::new(format!(
            "branchable text spans {lo:#x}..{hi:#x} ({} bytes), which straddles a \
             {region}-byte region boundary — SOG §4.8 requires every branch and its target to \
             share one 2 MiB region, so the text base must move to a region boundary \
             (plans/codegen-pareto.md decision 1754)",
            hi - lo,
            region = REGION_BYTES
        )));
    }
    Ok(())
}

fn place_pools(
    cursor: u64,
    sections: &[Section],
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Result<Option<(Vec<PoolPlacement>, u64, u64, u64)>, LayoutError> {
    if backings.is_empty() {
        return Ok(None);
    }
    let placed_end = sections
        .iter()
        .map(|s| s.base + s.size)
        .max()
        .unwrap_or(cursor);
    if cursor < placed_end {
        return Err(LayoutError::new(format!(
            "internal error: pool backing would be placed at {cursor:#x}, inside a section that \
             ends at {placed_end:#x}"
        )));
    }
    Ok(place_pools_unchecked(cursor, backings))
}

fn place_pools_unchecked(
    cursor: u64,
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Option<(Vec<PoolPlacement>, u64, u64, u64)> {
    if backings.is_empty() {
        return None;
    }
    let base = round_up(cursor, 8);
    let mut at = base;
    let mut out = Vec::with_capacity(backings.len());
    for b in backings.values() {
        at = round_up(at, b.align.max(1));
        out.push(PoolPlacement {
            backing: b.clone(),
            base: at,
        });
        at += b.bytes;
    }
    Some((out, base, at - base, at))
}

fn verify_pool_windows(sections: &[Section], pools: &[PoolPlacement]) -> Result<(), LayoutError> {
    if pools.is_empty() {
        return Ok(());
    }
    let Some(sec) = sections.iter().find(|s| s.name == "pooldata") else {
        return Err(LayoutError::new(
            "internal error: this image places pool windows but reserves no `pooldata` section",
        ));
    };
    let sec_end = sec.base + sec.size;
    for (i, p) in pools.iter().enumerate() {
        if p.backing.bytes == 0 {
            return Err(LayoutError::new(format!(
                "internal error: pool `{}` was placed with a zero-byte window",
                p.backing.name
            )));
        }
        let end = p.base.checked_add(p.backing.bytes).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: pool `{}`'s window overflows a u64",
                p.backing.name
            ))
        })?;
        if p.base < sec.base || end > sec_end {
            return Err(LayoutError::new(format!(
                "internal error: pool `{}`'s window [{:#x}, {end:#x}) is not inside the \
                 `pooldata` section [{:#x}, {sec_end:#x}) — plans/M7.md decision 5: a declared \
                 pool window is the whole of what a device can reach",
                p.backing.name, p.base, sec.base
            )));
        }
        for other in &pools[..i] {
            let other_end = other.base + other.backing.bytes;
            if p.base < other_end && other.base < end {
                return Err(LayoutError::new(format!(
                    "internal error: pools `{}` and `{}` were placed at overlapping windows",
                    other.backing.name, p.backing.name
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_ring_windows(
    pools: &[PoolPlacement],
    blk: &Option<BlkReport>,
) -> Result<(), LayoutError> {
    let Some(blk) = blk else {
        return Ok(());
    };
    let q = &blk.queue;
    let Some(pool) = pools.iter().find(|p| p.backing.name == q.pool_name) else {
        return Err(LayoutError::new(format!(
            "internal error: BlkQueue names pool `{}`, which has no placed window",
            q.pool_name
        )));
    };
    if pool.backing.device.is_none() {
        return Err(LayoutError::new(format!(
            "internal error: BlkQueue's pool `{}` is not device-reachable — decision 5: only \
             DMA pools are device-reachable memory",
            q.pool_name
        )));
    }
    let Some(placed) = crate::virtqueue::place_ring(pool.base, q.size) else {
        return Err(LayoutError::new(format!(
            "internal error: BlkQueue size {} is not a nonzero power of two",
            q.size
        )));
    };
    if placed.desc != q.desc
        || placed.avail != q.avail
        || placed.used != q.used
        || placed.doorbell != q.doorbell
    {
        return Err(LayoutError::new(format!(
            "internal error: reported BlkQueue ring addresses disagree with \
             virtqueue::place_ring(pool_base={:#x}, depth={}) — emitter and verifier must share \
             one derivation",
            pool.base, q.size
        )));
    }
    if placed.bytes > pool.backing.bytes {
        return Err(LayoutError::new(format!(
            "internal error: ring for queue depth {} needs {} bytes but pool `{}` only has {}",
            q.size, placed.bytes, q.pool_name, pool.backing.bytes
        )));
    }
    let pool_end = pool.base + pool.backing.bytes;
    for (what, addr, len) in [
        (
            "descriptor table",
            q.desc,
            crate::virtqueue::desc_bytes(q.size),
        ),
        (
            "available ring",
            q.avail,
            crate::virtqueue::avail_bytes(q.size),
        ),
        ("used ring", q.used, crate::virtqueue::used_bytes(q.size)),
        (
            "doorbell word",
            q.doorbell,
            crate::virtqueue::DOORBELL_BYTES,
        ),
    ] {
        let end = addr.checked_add(len).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: blk {what} address overflows a u64"
            ))
        })?;
        if addr < pool.base || end > pool_end {
            return Err(LayoutError::new(format!(
                "internal error: blk {what} at [{addr:#x}, {end:#x}) is not inside pool `{}`'s \
                 window [{:#x}, {pool_end:#x}) — plans/M7.md decision 5",
                q.pool_name, pool.base
            )));
        }
    }
    Ok(())
}

fn find_virtqueue_configure(
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<Option<(String, u16)>, LayoutError> {
    let mut found: Option<(String, u16)> = None;
    for prog in programs.values() {
        for site in &prog.virtqueue_configures {
            if found.is_some() {
                return Err(LayoutError::new(
                    "this image has more than one `VirtQueue.configure` call; machine v1's                      `blk` has exactly one queue"
                        .to_string(),
                ));
            }
            found = Some(site.clone());
        }
    }
    Ok(found)
}

pub fn derive_blk_report(
    pools: &[PoolPlacement],
    graph: &ImageGraph,
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<Option<BlkReport>, LayoutError> {
    let Some((pool_name, depth)) = find_virtqueue_configure(programs)? else {
        return Ok(None);
    };
    let Some(pool) = pools.iter().find(|p| p.backing.name == pool_name) else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` consumes pool `{pool_name}`, which has no placed backing"
        )));
    };
    let Some(blk_device) = pool.backing.device else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` consumes pool `{pool_name}`, which is not device-reachable \
             (`img.dma_pool(..., device=...)`); decision 5: only DMA pools are device-reachable"
        )));
    };
    let Some(placed) = crate::virtqueue::place_ring(pool.base, depth) else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure`'s depth={depth} is not a nonzero power of two"
        )));
    };
    let Some(needed) = crate::virtqueue::control_bytes_needed(depth) else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure`'s depth={depth} is not a nonzero power of two"
        )));
    };
    if needed > pool.backing.bytes {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` needs a {depth}-deep ring plus packaging ({needed} bytes) but \
             pool `{pool_name}` only reserves {pool_bytes} bytes — enlarge the `img.dma_pool` or \
             shrink the depth",
            pool_bytes = pool.backing.bytes,
        )));
    }
    let capacity = crate::eval::image_checks::blk_capacity_sectors(graph).ok_or_else(|| {
        LayoutError::new(
            "this image configures a virtio-blk queue but declares no `capacity_sectors=` on \
             its `img.device` (plans/M7.md item E1: capacity is an image-declared build constant)"
                .to_string(),
        )
    })?;
    let features = crate::eval::image_checks::blk_accepted_features(graph, programs)
        .map_err(|e| LayoutError::new(e.message))?;
    let vector = graph
        .devices
        .get(blk_device)
        .and_then(|d| crate::eval::image_checks::device_vector(&d.args));
    Ok(Some(BlkReport {
        device: blk_device,
        capacity_sectors: capacity,
        features,
        vector,
        queue: BlkQueueReport {
            index: 0,
            size: depth,
            desc: placed.desc,
            avail: placed.avail,
            used: placed.used,
            doorbell: placed.doorbell,
            pool_name,
        },
        descriptors_per_op: crate::virtqueue::DESCRIPTORS_PER_BLK_OP,
        occupancy_bound: crate::virtqueue::occupancy_bound(
            depth,
            crate::virtqueue::DESCRIPTORS_PER_BLK_OP,
        ),
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRegs {
    pub device: usize,
    pub device_type: String,
    pub driver: String,
    pub base: u64,
    pub size: u64,
}

fn device_register_windows(
    boot: Option<&BootCtx>,
) -> Result<Vec<(usize, String, String, u64)>, LayoutError> {
    use crate::eval::image::ImageDeclRef;
    use crate::eval::value::Value;

    let Some(b) = boot else {
        return Ok(Vec::new());
    };
    let layouts = closure_layout_types(b.modules, b.programs)?;
    let decls = closure_decl_items(b.modules)?;

    let mut out: Vec<(usize, String, String, u64)> = Vec::new();
    for d in &b.graph.drivers {
        let driver = crate::sema::types::render_type(&d.actor_type);
        let Some(Value::ImageDecl(ImageDeclRef::Device(idx))) = d
            .args
            .iter()
            .find(|a| a.label == "device")
            .map(|a| &a.value)
        else {
            continue;
        };
        let device_type = match b.graph.devices.get(*idx) {
            Some(dev) => crate::sema::types::render_type(&dev.device_type),
            None => {
                return Err(LayoutError::new(format!(
                    "internal error: `@driver` `{driver}` binds device#{idx}, which this image \
                     does not declare"
                )));
            }
        };
        let mut extent = 0u64;
        if let Some(mints) = crate::sema::types::driver_mmio_mints(&decls, &driver) {
            for name in mints {
                if let Some(l) = layouts.get(&name) {
                    extent = extent.max(crate::sema::types::mmio_consumed_end(l));
                }
            }
        }
        let size = round_up(extent.max(8), 8);
        out.push((*idx, device_type, driver, size));
    }
    out.sort_by_key(|(idx, _, _, _)| *idx);
    Ok(out)
}

fn place_device_regs(
    cursor: u64,
    windows: &[(usize, String, String, u64)],
) -> Option<(Vec<DeviceRegs>, u64, u64, u64)> {
    if windows.is_empty() {
        return None;
    }
    let base = round_up(cursor, 8);
    let mut at = base;
    let mut out = Vec::with_capacity(windows.len());
    for (device, device_type, driver, size) in windows {
        out.push(DeviceRegs {
            device: *device,
            device_type: device_type.clone(),
            driver: driver.clone(),
            base: at,
            size: *size,
        });
        at += size;
    }
    Some((out, base, at - base, at))
}

fn verify_device_windows(sections: &[Section], regs: &[DeviceRegs]) -> Result<(), LayoutError> {
    if regs.is_empty() {
        return Ok(());
    }
    let Some(sec) = sections.iter().find(|s| s.name == "devregs") else {
        return Err(LayoutError::new(
            "internal error: this image places device register windows but reserves no `devregs` \
             section",
        ));
    };
    let sec_end = sec.base + sec.size;
    for (i, r) in regs.iter().enumerate() {
        if r.size == 0 {
            return Err(LayoutError::new(format!(
                "internal error: device#{} was placed with a zero-byte register window",
                r.device
            )));
        }
        let end = r.base + r.size;
        if r.base < sec.base || end > sec_end {
            return Err(LayoutError::new(format!(
                "internal error: device#{}'s register window [{:#x}, {end:#x}) is not inside the \
                 `devregs` section [{:#x}, {sec_end:#x})",
                r.device, r.base, sec.base
            )));
        }
        for other in &regs[..i] {
            let other_end = other.base + other.size;
            if r.base < other_end && other.base < end {
                return Err(LayoutError::new(format!(
                    "internal error: device#{} and device#{} were placed at overlapping register \
                     windows",
                    other.device, r.device
                )));
            }
        }
    }
    Ok(())
}

fn closure_decl_items(
    modules: &BTreeMap<String, Module>,
) -> Result<Vec<crate::sema::types::DeclItem>, LayoutError> {
    let imported = closure_imported_types(modules)
        .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?;
    let mut out = Vec::new();
    for (addr, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?;
        out.extend(
            crate::sema::types::declare_with_imports(&specialized, &imported[addr])
                .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?,
        );
    }
    Ok(out)
}

fn closure_layout_types(
    modules: &BTreeMap<String, Module>,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<BTreeMap<String, crate::sema::types::LayoutType>, LayoutError> {
    let mut out = BTreeMap::new();
    for (key, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        let mut layouts = crate::sema::types::check_layouts(&specialized)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        let Some(program) = programs.get(key) else {
            return Err(LayoutError::new(format!(
                "internal error: module `{key}` is in the build closure without a typed program, \
                 so `complete_layouts` cannot resolve a `@layout(runtime)` const array length \
                 (plans/M10.md item E1)"
            )));
        };
        crate::sema::types::complete_layouts(&specialized, program, &mut layouts)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        for l in layouts {
            out.insert(l.name.clone(), l);
        }
    }
    Ok(out)
}

fn image_pool_backings(
    boot: Option<&BootCtx>,
) -> Result<BTreeMap<String, crate::eval::image_checks::PoolBacking>, LayoutError> {
    let Some(b) = boot else {
        return Ok(BTreeMap::new());
    };
    let layouts = closure_layout_types(b.modules, b.programs)?;
    crate::eval::image_checks::pool_backings(b.graph, &layouts).map_err(|e| {
        LayoutError::new(format!(
            "internal error: a pool declaration this image's own graph check accepted cannot be \
             placed: {}",
            e.message
        ))
    })
}

pub fn layout_program(
    program: &CodegenProgram,
    boot: Option<BootCtx>,
) -> Result<ImageLayout, LayoutError> {
    layout_program_inner(program, boot)
}

fn layout_program_inner(
    program: &CodegenProgram,
    boot: Option<BootCtx>,
) -> Result<ImageLayout, LayoutError> {
    let _late_address_relax = crate::codegen::late_address_relax_guard();
    let image_base = machine_layout::IMAGE_BASE;

    let mut wiring: Option<RuntimeWiring> = match &boot {
        Some(b) => RuntimeWiring::derive(b)?,
        None => None,
    };

    let mut rodata_entries: Vec<Vec<u8>> = program.rodata.clone();
    let mut rodata_cursor: usize = rodata_entries.iter().map(Vec::len).sum();
    if let Some(w) = wiring.as_mut() {
        intern_fallible_init_abort_messages(w, &mut rodata_entries, &mut rodata_cursor);
    }

    let mut program_owned;
    let program = if let Some(w) = wiring.as_ref() {
        program_owned = program.clone();
        apply_resume_remaps(&mut program_owned, w);
        inject_rt_enqueue_and_dispatch_fns(&mut program_owned, w)?;
        inject_rt_cross_core_fns(&mut program_owned, w)?;
        inject_boot_init_fn(&mut program_owned, w);
        inject_checkpoint_irq_fns(&mut program_owned, w);
        &program_owned
    } else {
        program
    };

    verify_conventions_after_layout(program)?;

    // Relax only self-contained, value-only sites before assigning final
    // function bases.  The relaxation pass freezes any function with a
    // relocation or control transfer, so every surviving relocation index is
    // still local to an unchanged wide body.  All later section addresses and
    // patches are then computed from this one final program.
    let relaxed_program = crate::relax::relax_immediates(program)
        .map_err(|e| LayoutError::new(format!("late immediate relaxation: {e}")))?;
    let program = &relaxed_program.program;

    let entry_words = build_entry_stub();

    let mut code_words: Vec<u32> =
        Vec::with_capacity(program.fns.values().map(|f| f.code.len()).sum());
    let mut fn_word_base: BTreeMap<String, usize> = BTreeMap::new();
    for (key, f) in &program.fns {
        fn_word_base.insert(key.clone(), code_words.len());
        for ew in &f.code {
            code_words.push(ew.word);
        }
    }
    let rodata_bytes: Vec<u8> = rodata_entries
        .iter()
        .flat_map(|entry| entry.iter().copied())
        .collect();
    let runtime: Option<&RuntimeTables> = wiring.as_ref().map(|w| &w.tables);

    let mut abort_fixed_words = Vec::new();
    push_halt(&mut abort_fixed_words, EXIT_CODE_ABORT_FIXED);
    let mut abort_val_words = Vec::new();
    push_halt(&mut abort_val_words, EXIT_CODE_ABORT_VAL);
    let checkpoint_shape = group_service_shape(runtime);
    let (irq_shape, wake_shape) = checkpoint_irq_shape(boot.as_ref(), None, runtime);
    let link_cp_body = runtime.is_some();
    let checkpoint_block = build_checkpoint_and_vector_stub_ex(
        checkpoint_shape.as_ref(),
        &irq_shape,
        &wake_shape,
        link_cp_body,
    );
    let checkpoint_words = checkpoint_block.words;
    let checkpoint_service_word = checkpoint_block.checkpoint_service_word;
    let checkpoint_relocs_shape = checkpoint_block.relocs;

    let rtcode_words_len = 0usize;

    let mut cursor = image_base;

    let entry_base = cursor;
    let entry_size = (entry_words.len() * 4) as u64;
    cursor += entry_size;

    cursor = round_up(cursor, 4);
    let code_base = cursor;
    let code_size = (code_words.len() * 4) as u64;
    cursor += code_size;

    let rodata_base = if rodata_bytes.is_empty() {
        None
    } else {
        cursor = round_up(cursor, 8);
        let base = cursor;
        cursor += rodata_bytes.len() as u64;
        Some(base)
    };

    cursor = round_up(cursor, 4);
    let abort_fixed_base = cursor;
    cursor += (abort_fixed_words.len() * 4) as u64;
    let abort_val_base = cursor;
    cursor += (abort_val_words.len() * 4) as u64;
    let abort_size = cursor - abort_fixed_base;

    let checkpoint_base = cursor;
    let checkpoint_size = (checkpoint_words.len() * 4) as u64;
    cursor += checkpoint_size;
    let checkpoint_service_addr = checkpoint_base + (checkpoint_service_word as u64) * 4;

    let rtcode_base = if rtcode_words_len > 0 {
        let base = cursor;
        cursor += (rtcode_words_len * 4) as u64;
        Some(base)
    } else {
        None
    };

    let mut sections = vec![
        Section {
            name: "entry",
            base: entry_base,
            size: entry_size,
        },
        Section {
            name: "code",
            base: code_base,
            size: code_size,
        },
    ];
    if let Some(rb) = rodata_base {
        sections.push(Section {
            name: "rodata",
            base: rb,
            size: rodata_bytes.len() as u64,
        });
    }
    sections.push(Section {
        name: "abort",
        base: abort_fixed_base,
        size: abort_size,
    });
    sections.push(Section {
        name: "checkpoint",
        base: checkpoint_base,
        size: checkpoint_size,
    });
    if let Some(base) = rtcode_base {
        sections.push(Section {
            name: "rtcode",
            base,
            size: (rtcode_words_len * 4) as u64,
        });
    }

    let rtdata_base = if let Some(tables) = runtime.filter(|t| t.total_bytes > 0) {
        let base = steer_rtdata_base(cursor, tables)?;
        cursor = base;
        sections.push(Section {
            name: "rtdata",
            base,
            size: tables.total_bytes,
        });
        cursor += tables.total_bytes;
        Some(base)
    } else {
        None
    };

    let device_windows = device_register_windows(boot.as_ref())?;
    let placed_regs = place_device_regs(cursor, &device_windows);
    let device_regs: Vec<DeviceRegs> = match &placed_regs {
        Some((regs, base, size, end)) => {
            sections.push(Section {
                name: "devregs",
                base: *base,
                size: *size,
            });
            cursor = *end;
            regs.clone()
        }
        None => Vec::new(),
    };

    let pool_backings = image_pool_backings(boot.as_ref())?;
    let placed_pools = place_pools(cursor, &sections, &pool_backings)?;
    let pools: Vec<PoolPlacement> = match &placed_pools {
        Some((pools, base, size, end)) => {
            sections.push(Section {
                name: "pooldata",
                base: *base,
                size: *size,
            });
            cursor = *end;
            pools.clone()
        }
        None => Vec::new(),
    };
    let _ = cursor;

    let runtime_live = runtime.filter(|t| t.total_bytes > 0);
    let placement = match (rtdata_base, runtime_live) {
        (Some(base), Some(tables)) => Some(place_runtime_tables(base, tables)),
        _ => None,
    };
    let (mut checkpoint_words, checkpoint_relocs) = match (&placement, runtime_live) {
        (Some(pl), Some(tables)) => {
            let (irq_real, wake_real) = checkpoint_irq_shape(boot.as_ref(), Some(pl), Some(tables));
            let real = build_checkpoint_and_vector_stub_ex(
                group_service_ctx(pl, tables).as_ref(),
                &irq_real,
                &wake_real,
                true,
            );
            if real.words.len() != checkpoint_words.len() {
                return Err(LayoutError::new(
                    "internal error: the checkpoint block's own word count changed between its \
                     sizing pass and its real-address pass",
                ));
            }
            (real.words, real.relocs)
        }
        _ => (checkpoint_words, checkpoint_relocs_shape),
    };
    for reloc in &checkpoint_relocs {
        match reloc {
            Reloc::Call { word, key } => {
                let target_base = *fn_word_base.get(key).ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: checkpoint dispatch target `{key}` was never codegen'd"
                    ))
                })?;
                let this_addr = checkpoint_base + (*word as u64) * 4;
                let target_addr = code_base + (target_base as u64) * 4;
                patch_bl(&mut checkpoint_words, *word, this_addr, target_addr)?;
            }
            other => {
                return Err(LayoutError::new(format!(
                    "internal error: checkpoint block emitted unexpected reloc {other:?}"
                )));
            }
        }
    }
    // ADR is the one-word short form of the existing two-word ADRP+ADD
    // relocation.  If the final addresses prove it out of range, grow only
    // a straight-line function and restart layout.  Restarting is important:
    // the insertion moves every later function and therefore all later
    // relocation PCs.  Functions containing local control transfers remain
    // wide/fail-closed rather than leaving stale branch displacements.
    let mut expand_adr = BTreeMap::<String, BTreeSet<usize>>::new();
    if rodata_base.is_some() {
        for (key, f) in &program.fns {
            if f.code.iter().any(|word| {
                matches!(
                    word.rule,
                    crate::cost::CostRule::Branch
                        | crate::cost::CostRule::Call
                        | crate::cost::CostRule::Abort
                        | crate::cost::CostRule::AbortVal
                )
            }) {
                continue;
            }
            let base = fn_word_base[key];
            for reloc in &f.relocs {
                let Reloc::RodataAdr { word, byte_offset } = reloc else {
                    continue;
                };
                let rb = rodata_base.expect("checked above");
                let pc = code_base + ((base + *word) as u64) * 4;
                let target = rb + *byte_offset as u64;
                let delta = target as i64 - pc as i64;
                if !(-ADR_HALF_RANGE_BYTES..ADR_HALF_RANGE_BYTES).contains(&delta) {
                    expand_adr.entry(key.clone()).or_default().insert(*word);
                }
            }
        }
    }
    if !expand_adr.is_empty() {
        let expanded = expand_rodata_adr_sites(program, &expand_adr)?;
        return layout_program_inner(&expanded, boot);
    }

    let empty_symbols = BTreeMap::new();
    let glue_symbols: &BTreeMap<String, usize> = &empty_symbols;
    let mut all_code_words = code_words;
    for (key, f) in &program.fns {
        let base = fn_word_base[key];
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    let redirect = resolve_cross_core_edge(key, target, wiring.as_ref())?;
                    let xreply = wiring.as_ref().and_then(|w| resolve_xreply_edge(target, w));
                    let target_owned: String =
                        redirect.or(xreply).unwrap_or_else(|| target.clone());
                    let target = target_owned.as_str();
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = if let Some(target_base) = fn_word_base.get(target) {
                        code_base + (*target_base as u64) * 4
                    } else if let (Some(rc), Some(glue_word)) =
                        (rtcode_base, glue_symbols.get(target))
                    {
                        rc + (*glue_word as u64) * 4
                    } else {
                        return Err(unresolved_call_target(
                            target,
                            boot.as_ref().map(|b| b.graph),
                        ));
                    };
                    patch_bl(&mut all_code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::Rodata exists but the rodata section is empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word_adrp) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adrp_add(
                        &mut all_code_words,
                        base + word_adrp,
                        this_addr,
                        target_addr,
                    )?;
                }
                Reloc::RodataAdr { word, byte_offset } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RodataAdr exists but the rodata section is \
                             empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adr(&mut all_code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::AbortFixed { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    patch_bl(
                        &mut all_code_words,
                        base + word,
                        this_addr,
                        abort_fixed_base,
                    )?;
                }
                Reloc::AbortVal { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    patch_bl(&mut all_code_words, base + word, this_addr, abort_val_base)?;
                }
                Reloc::CheckpointService { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    patch_bl(
                        &mut all_code_words,
                        base + word,
                        this_addr,
                        checkpoint_service_addr,
                    )?;
                }
                Reloc::TurnFrameAddr { word, key: fn_key } => {
                    let addr = placement
                        .as_ref()
                        .zip(runtime_live)
                        .and_then(|(p, t)| p.turn_area_for(fn_key, t))
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: async fn `{fn_key}` needs a turn area but this \
                                 image's runtime tables never sized one"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::TurnIdImm { word, key: fn_key } => {
                    let id = placement
                        .as_ref()
                        .zip(runtime_live)
                        .and_then(|(p, t)| p.turn_id_for(fn_key, t))
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: async fn `{fn_key}` needs a turn id but this \
                                 image's runtime tables never sized one"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, id.get() as u64);
                }
                Reloc::TurnsBase { word } => {
                    let addr = placement
                        .as_ref()
                        .map(|p| p.turns_base)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::TurnStride { word } => {
                    let stride = placement
                        .as_ref()
                        .map(|p| p.turn_stride)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut all_code_words, base + word, stride);
                }
                Reloc::GroupArenaBase { word } => {
                    let addr = placement.as_ref().map(|p| p.group_arena).ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a `with group` op needs the group arena but this \
                             image's runtime tables never sized one"
                                .to_string(),
                        )
                    })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::IrqVector { word, driver } => {
                    let vector = driver_irq_vector(boot.as_ref().map(|b| b.graph), driver)?;
                    patch_load_imm_words(&mut all_code_words, base + word, vector);
                }
                Reloc::WakePending { word, driver } => {
                    let (p, t) = match (placement.as_ref(), runtime_live) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(wake_needs_rtdata(driver));
                        }
                    };
                    let addr = driver_wake_pending_addr(p, t, driver)?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::MailboxAddr { word, actor, field } => {
                    let (p, t) = match (placement.as_ref(), runtime_live) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::MailboxAddr exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addrs = resolve_mailbox_actor_addrs(p, t, actor).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::MailboxAddr names actor `{actor}`, which this \
                             image's runtime tables never placed a mailbox for"
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::MailboxField::Ring => addrs.ring,
                        crate::codegen::MailboxField::Head => addrs.head,
                        crate::codegen::MailboxField::Tail => addrs.tail,
                        crate::codegen::MailboxField::Count => addrs.count,
                        crate::codegen::MailboxField::State => addrs.state,
                        crate::codegen::MailboxField::Turn => addrs.turn,
                    };
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::RrCursor { word, core } => {
                    let p = placement.as_ref().ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RrCursor exists but this image has no \
                             runtime placement",
                        )
                    })?;
                    let addr = p.rr_cursors.get(*core).copied().ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::RrCursor names core {core}, but this image \
                             only placed {} rr_cursor(s)",
                            p.rr_cursors.len()
                        ))
                    })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::RingAddr {
                    word,
                    ring_index,
                    field,
                } => {
                    let p = placement.as_ref().ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RingAddr exists but this image has no \
                             runtime placement",
                        )
                    })?;
                    let addrs = p.rings.get(*ring_index).copied().ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::RingAddr names ring_index {ring_index}, but \
                             this image only placed {} ring(s)",
                            p.rings.len()
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::RingField::Ring => addrs.ring,
                        crate::codegen::RingField::Head => addrs.head,
                        crate::codegen::RingField::Tail => addrs.tail,
                        crate::codegen::RingField::Count => addrs.count,
                    };
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::DriverState { word, driver } => {
                    let (p, t) = match (placement.as_ref(), runtime_live) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::DriverState exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addr = driver_state_addr(p, t, driver)?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::DeviceRegsBase { word, device } => {
                    let addr = device_regs
                        .iter()
                        .find(|r| r.device == *device)
                        .map(|r| r.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::DeviceRegsBase names device#{device}, \
                                 which this image never placed"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::PoolBase { word, pool } => {
                    let addr = pools
                        .iter()
                        .find(|p| &p.backing.name == pool)
                        .map(|p| p.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::PoolBase names pool `{pool}`, which this \
                                 image never placed"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::PoolSlot {
                    word,
                    pool,
                    index,
                    slot_bytes,
                } => {
                    let base_addr = pools
                        .iter()
                        .find(|p| &p.backing.name == pool)
                        .map(|p| p.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::PoolSlot names pool `{pool}`, which this \
                                 image never placed"
                            ))
                        })?;
                    let addr = base_addr + *index * *slot_bytes;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
            }
        }
    }

    let rtcode_words: Vec<u32> = Vec::new();

    let mut blob = Vec::with_capacity(
        sections
            .iter()
            .map(|s| (s.base + s.size).saturating_sub(image_base) as usize)
            .max()
            .unwrap_or(0),
    );
    for w in &entry_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    pad_to(&mut blob, image_base, code_base);
    for w in &all_code_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    if let Some(rb) = rodata_base {
        pad_to(&mut blob, image_base, rb);
        blob.extend_from_slice(&rodata_bytes);
    }
    pad_to(&mut blob, image_base, abort_fixed_base);
    for w in &abort_fixed_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    for w in &abort_val_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    pad_to(&mut blob, image_base, checkpoint_base);
    for w in &checkpoint_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    if let Some(rc) = rtcode_base {
        pad_to(&mut blob, image_base, rc);
        for w in &rtcode_words {
            blob.extend_from_slice(&w.to_le_bytes());
        }
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, runtime.filter(|t| t.total_bytes > 0)) {
        pad_to(&mut blob, image_base, rb);
        blob.resize(blob.len() + tables.total_bytes as usize, 0);
    }
    if let Some((_, base, size, _)) = &placed_regs {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }
    if let Some((_, base, size, _)) = &placed_pools {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;
    verify_pool_windows(&sections, &pools)?;
    verify_device_windows(&sections, &device_regs)?;

    let irq_host_injects = build_irq_host_injects(boot.as_ref(), &device_regs);
    let mut core_entries: Vec<(usize, u64)> = match (wiring.as_ref(), code_base) {
        (Some(w), cb) if w.tables.cores > 1 => (1..w.tables.cores)
            .filter_map(|core| {
                let key = crate::codegen::rt_secondary_core_entry_symbol(core);
                fn_word_base
                    .get(&key)
                    .map(|&word| (core, cb + (word as u64) * 4))
            })
            .collect(),
        _ => Vec::new(),
    };
    let cores = wiring.as_ref().map(|w| w.tables.cores).unwrap_or(1).max(1);

    // Construct the one final-address representation used by cost and
    // diagnostics.  Relocation patching changed only `word`, so all original
    // EmittedWord metadata remains attached to the linked functions.
    let entry_id = 0usize;
    let code_id = 1usize;
    let mut linked_sections = vec![
        crate::linked::LinkedSection {
            id: entry_id,
            name: "entry".to_string(),
            byte_address: entry_base,
            executable: true,
            code: crate::linked::synthetic_words(&entry_words, "__image_entry"),
            raw_bytes: Vec::new(),
            padding_before: 0,
        },
        crate::linked::LinkedSection {
            id: code_id,
            name: "code".to_string(),
            byte_address: code_base,
            executable: true,
            code: Vec::new(),
            raw_bytes: Vec::new(),
            padding_before: code_base.saturating_sub(entry_base + entry_size),
        },
    ];
    let mut linked_fns = BTreeMap::new();
    let entry_code = crate::linked::synthetic_words(&entry_words, "__image_entry");
    linked_fns.insert(
        "__image_entry".to_string(),
        crate::linked::LinkedFn {
            key: "__image_entry".to_string(),
            section: entry_id,
            byte_address: entry_base,
            origin_word_ranges: crate::linked::default_origin_ranges(&entry_code),
            code: entry_code,
            relocs: Vec::new(),
            frame_size: 0,
        },
    );
    for (key, f) in program.fns.iter() {
        let base = *fn_word_base
            .get(key)
            .ok_or_else(|| LayoutError::new(format!("linked function `{key}` has no code base")))?;
        let mut fn_code = f.code.clone();
        crate::linked::complete_memory_metadata(key, &mut fn_code);
        for (i, ew) in fn_code.iter_mut().enumerate() {
            ew.word = all_code_words[base + i];
        }
        let address = code_base + (base as u64) * 4;
        linked_sections[1].code.extend(fn_code.iter().cloned());
        linked_fns.insert(
            key.clone(),
            crate::linked::LinkedFn {
                key: key.clone(),
                section: code_id,
                byte_address: address,
                origin_word_ranges: crate::linked::recorded_origin_ranges(
                    &program.origin_spans,
                    key,
                    &fn_code,
                ),
                code: fn_code,
                relocs: f.relocs.clone(),
                frame_size: f.frame_size as u64,
            },
        );
    }
    let mut next_section = 2usize;
    let mut add_exec = |key: &str, address: u64, words: &[u32], frame_size: u64| {
        let id = next_section;
        next_section += 1;
        let code = crate::linked::synthetic_words(words, key);
        linked_sections.push(crate::linked::LinkedSection {
            id,
            name: key.to_string(),
            byte_address: address,
            executable: true,
            code: code.clone(),
            raw_bytes: Vec::new(),
            padding_before: 0,
        });
        linked_fns.insert(
            key.to_string(),
            crate::linked::LinkedFn {
                key: key.to_string(),
                section: id,
                byte_address: address,
                origin_word_ranges: crate::linked::default_origin_ranges(&code),
                code,
                relocs: Vec::new(),
                frame_size,
            },
        );
    };
    add_exec(
        "__image_abort_fixed",
        abort_fixed_base,
        &abort_fixed_words,
        0,
    );
    add_exec("__image_abort_value", abort_val_base, &abort_val_words, 0);
    add_exec(
        "__image_checkpoint_vector",
        checkpoint_base,
        &checkpoint_words,
        0,
    );
    if let Some(base) = rtcode_base {
        add_exec("__image_rtcode", base, &rtcode_words, 0);
    }
    if let Some(rb) = rodata_base {
        linked_sections.push(crate::linked::LinkedSection {
            id: next_section,
            name: "rodata".to_string(),
            byte_address: rb,
            executable: false,
            code: Vec::new(),
            raw_bytes: rodata_bytes.clone(),
            padding_before: 0,
        });
        next_section += 1;
    }
    for section in &sections {
        if matches!(
            section.name,
            "entry" | "code" | "rodata" | "abort" | "checkpoint" | "rtcode"
        ) {
            continue;
        }
        linked_sections.push(crate::linked::LinkedSection {
            id: next_section,
            name: section.name.to_string(),
            byte_address: section.base,
            executable: false,
            code: Vec::new(),
            raw_bytes: vec![0; section.size as usize],
            padding_before: 0,
        });
        next_section += 1;
    }
    let mut linked =
        crate::linked::LinkedProgram::from_parts(linked_sections, linked_fns, image_base)
            .map_err(LayoutError::new)?;
    let (relaxed_linked, _) = crate::relax::relax_linked_addresses(&linked)
        .map_err(|e| LayoutError::new(format!("late address relaxation: {e}")))?;
    linked = relaxed_linked;
    if let Some(boot) = boot.as_ref() {
        // Persistent Flow storage exists once per placed turn record, not once
        // per async function.  Actor/driver records reserve the maximum frame
        // of their methods; free async functions each own one record.
        linked.async_frame_total_bytes =
            runtime
                .map(|tables| {
                    tables
                        .actors
                        .iter()
                        .map(|actor| {
                            actor
                                .frame_size
                                .saturating_sub(crate::codegen::TURN_RECORD_SIZE)
                        })
                        .chain(tables.drivers.iter().filter_map(|driver| {
                            driver.mailbox.as_ref().map(|mailbox| {
                                mailbox
                                    .frame_size
                                    .saturating_sub(crate::codegen::TURN_RECORD_SIZE)
                            })
                        }))
                        .chain(tables.free_turns.iter().map(|(_, bytes)| {
                            bytes.saturating_sub(crate::codegen::TURN_RECORD_SIZE)
                        }))
                        .sum()
                })
                .unwrap_or(0);
        linked.sync_frame_max_bytes = linked
            .fns
            .iter()
            .filter(|(key, _)| !boot.flow.fns.contains_key(*key))
            .map(|(_, f)| f.frame_size)
            .max()
            .unwrap_or(0);
    }
    for (core, address) in &mut core_entries {
        let key = crate::codegen::rt_secondary_core_entry_symbol(*core);
        if let Some(function) = linked.fns.get(&key) {
            *address = function.byte_address;
        }
    }
    crate::cost::audit::audit_linked(&linked).map_err(LayoutError::new)?;
    let linked_blob = linked.serialize(image_base).map_err(LayoutError::new)?;
    for section in &mut sections {
        if let Some(linked_section) = linked
            .sections
            .iter()
            .find(|candidate| candidate.name == section.name)
        {
            section.base = linked_section.byte_address;
            section.size = linked_section.payload_bytes();
            continue;
        }
        let aliases: &[&str] = match section.name {
            "abort" => &["__image_abort_fixed", "__image_abort_value"],
            "checkpoint" => &["__image_checkpoint_vector"],
            "rtcode" => &["__image_rtcode"],
            _ => &[],
        };
        let owned: Vec<&crate::linked::LinkedSection> = linked
            .sections
            .iter()
            .filter(|candidate| aliases.contains(&candidate.name.as_str()))
            .collect();
        if !owned.is_empty() {
            section.base = owned
                .iter()
                .map(|candidate| candidate.byte_address)
                .min()
                .unwrap();
            section.size = owned
                .iter()
                .map(|candidate| candidate.payload_bytes())
                .sum();
        }
    }
    verify_section_sizes(&sections, image_base, linked_blob.len() as u64)?;

    Ok(ImageLayout {
        blob: linked_blob,
        linked: Some(linked),
        entry: entry_base,
        sections,
        runtime: runtime.cloned(),
        pools,
        device_regs,
        blk: None,
        irq_host_injects,
        core_entries,
        cores,
        placed_statics: Vec::new(),
    })
}

fn closure_imported_types(
    modules: &BTreeMap<String, Module>,
) -> Result<BTreeMap<String, crate::sema::types::ImportedTypes>, SemaError> {
    let mut specialized: BTreeMap<String, Module> = BTreeMap::new();
    for (addr, m) in modules {
        specialized.insert(addr.clone(), crate::sema::specialize::specialize(m)?);
    }
    let by_addr: Vec<(Vec<String>, &Module)> = specialized
        .iter()
        .map(|(addr, m)| (addr.split('.').map(str::to_string).collect(), m))
        .collect();
    let shapes = crate::sema::imports::closure_type_shapes(&by_addr);
    Ok(specialized
        .iter()
        .map(|(addr, m)| {
            let mut imported = crate::sema::imports::imported_type_shapes(m, &shapes);
            if crate::loader::module_mentions_time(m) {
                for name in ["Duration", "Instant"] {
                    imported.entry(name.to_string()).or_insert(0);
                }
            }
            (addr.clone(), imported)
        })
        .collect())
}

pub fn merge_layout_ctx(modules: &BTreeMap<String, Module>) -> Result<LayoutCtx, SemaError> {
    let imported = closure_imported_types(modules)?;
    let mut merged = LayoutCtx::default();
    for (addr, module) in modules {
        let ctx = crate::mwir::build_layout_ctx(module, &imported[addr])?;
        merged.structs.extend(ctx.structs);
        merged.enums.extend(ctx.enums);
        merged.struct_field_names.extend(ctx.struct_field_names);
    }
    install_aliased_import_layouts(&mut merged, modules)?;
    Ok(merged)
}

fn install_aliased_import_layouts(
    ctx: &mut LayoutCtx,
    modules: &BTreeMap<String, Module>,
) -> Result<(), SemaError> {
    let mut specialized: BTreeMap<String, Module> = BTreeMap::new();
    for (addr, m) in modules {
        specialized.insert(addr.clone(), crate::sema::specialize::specialize(m)?);
    }
    let by_addr: Vec<(Vec<String>, &Module)> = specialized
        .iter()
        .map(|(addr, m)| (addr.split('.').map(str::to_string).collect(), m))
        .collect();
    let shapes = crate::sema::imports::closure_type_shapes(&by_addr);
    for module in specialized.values() {
        let targets = crate::sema::imports::imported_type_targets(module, &shapes);
        let mut subs_by_exporter: BTreeMap<Vec<String>, BTreeMap<String, String>> = BTreeMap::new();
        for (local, (target_mod, target_name)) in &targets {
            if local != target_name {
                subs_by_exporter
                    .entry(target_mod.clone())
                    .or_default()
                    .insert(target_name.clone(), local.clone());
            }
        }
        for (local, (target_mod, target_name)) in &targets {
            if local == target_name {
                continue;
            }
            let subs = &subs_by_exporter[target_mod];
            if let Some(mut fields) = ctx.structs.get(target_name).cloned() {
                for f in &mut fields {
                    crate::sema::types::rekey_type_names(f, subs);
                }
                ctx.structs.insert(local.clone(), fields);
                if let Some(names) = ctx.struct_field_names.get(target_name).cloned() {
                    ctx.struct_field_names.insert(local.clone(), names);
                }
            }
            if let Some(mut payloads) = ctx.enums.get(target_name).cloned() {
                for payload in &mut payloads {
                    for t in payload {
                        crate::sema::types::rekey_type_names(t, subs);
                    }
                }
                ctx.enums.insert(local.clone(), payloads);
            }
        }
    }
    Ok(())
}

pub fn enrich_layout_ctx_with_instantiations(
    ctx: &mut LayoutCtx,
    programs: &BTreeMap<String, TypedProgram>,
) {
    use crate::sema::typed::TypedInstantiation;
    for typed in programs.values() {
        let own = typed.instantiations.iter();
        let imported = typed.imported.instantiations.iter();
        for (key, inst) in own.chain(imported) {
            let TypedInstantiation::Struct(s) = inst else {
                continue;
            };
            let display = key.strip_prefix("struct:").unwrap_or(key.as_str());
            let fields: Vec<crate::sema::types::Type> = s
                .fields
                .iter()
                .filter_map(|n| s.field_types.get(n).cloned())
                .collect();
            ctx.struct_field_names
                .insert(display.to_string(), s.fields.clone());
            ctx.structs.insert(display.to_string(), fields);
        }
    }
}

pub fn merge_mwir_programs(programs: Vec<mwir::MwirProgram>) -> mwir::MwirProgram {
    let mut merged_fns: BTreeMap<String, mwir::MwirFn> = BTreeMap::new();
    let mut merged_rodata: Vec<Vec<u8>> = Vec::new();
    for p in programs {
        let offset = merged_rodata.len();
        merged_rodata.extend(p.rodata);
        for (key, mut f) in p.fns {
            if offset != 0 {
                for inst in &mut f.body {
                    if let mwir::Inst::ConstText { data, .. } = inst {
                        *data += offset;
                    }
                }
            }
            merged_fns.insert(key, f);
        }
    }
    mwir::MwirProgram {
        fns: merged_fns,
        rodata: merged_rodata,
    }
}

pub struct ImageCodegen {
    pub program: CodegenProgram,
    pub modules: BTreeMap<String, Module>,
    pub programs: BTreeMap<String, TypedProgram>,
    pub flow: FlowWirProgram,
    pub async_frames: BTreeMap<String, u64>,
    pub group_child_index: BTreeMap<String, usize>,
    pub layout_ctx: LayoutCtx,
    pub rtconfig_text: String,
}

pub fn lower_and_codegen_image(
    modules: &BTreeMap<String, Module>,
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    runtime_tests: &[String],
    async_tests: &BTreeSet<String>,
    emit_comptime_tests: bool,
) -> Result<ImageCodegen, String> {
    let capacity = crate::eval::image_checks::blk_capacity_sectors(graph);
    let pixels_skeleton = if graph.renderers.is_empty() {
        None
    } else {
        let owner = programs
            .values()
            .find(|program| program.image_fn.is_some())
            .ok_or_else(|| "pixels: image owner program is missing".to_string())?;
        Some(crate::pixels::compile_plane_skeleton(
            owner, programs, graph,
        )?)
    };
    let mut layout_ctx = layout_ctx.clone();
    enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);

    let reach_opts = crate::lower::LowerOpts {
        emit_comptime_tests,
        only: None,
    };
    let reachable = crate::lower::guest_reachable_keys_closure(programs, &reach_opts);
    let derive_opts = crate::lower::LowerOpts {
        emit_comptime_tests,
        only: Some(reachable),
    };
    let flow_derive = lower_flow_closure(programs, capacity, &derive_opts)?;
    let async_frames_derive =
        crate::codegen::async_frame_sizes(&flow_derive, &layout_ctx).map_err(|e| e.message)?;
    let (group_child_index_derive, _) =
        crate::codegen::compute_group_child_indices(&flow_derive).map_err(|e| e.message)?;
    let boot = BootCtx {
        graph,
        modules,
        programs,
        layout_ctx: &layout_ctx,
        async_frames: &async_frames_derive,
        group_child_index: &group_child_index_derive,
        flow: &flow_derive,
    };
    let wiring = RuntimeWiring::derive(&boot).map_err(|e| e.message)?;
    let tests = test_runner_facts(
        runtime_tests,
        async_tests,
        wiring.as_ref().map(|w| &w.tables),
    );
    let need_live = wiring.is_some() || !tests.is_empty() || pixels_skeleton.is_some();

    let (live_modules, live_programs, rtconfig_text, force_opts) = if need_live {
        let (text, force_opts) = generate_live_rtconfig(wiring.as_ref(), &tests)?;
        let (mods, progs) = recheck_with_live_rtconfig(modules, &text)?;
        (mods, progs, text, force_opts)
    } else {
        (
            modules.clone(),
            programs.clone(),
            String::new(),
            crate::lower::ImageForceRootOpts::default(),
        )
    };

    let mut only = crate::lower::guest_reachable_keys_closure(&live_programs, &reach_opts);
    crate::lower::seed_image_force_roots(&mut only, &live_programs, force_opts);
    let lower_opts = crate::lower::LowerOpts {
        emit_comptime_tests,
        only: Some(only),
    };
    let mwir = lower_mwir_closure(&live_programs, capacity, &lower_opts)?;
    let flow = lower_flow_closure(&live_programs, capacity, &lower_opts)?;
    let method_index =
        actor_method_index_tables(&live_modules, &layout_ctx).map_err(|e| e.message)?;
    let group_arena_capacity = count_with_group_sites(&live_modules);
    let enqueue_specs = mailbox_enqueue_specs(graph, &live_modules, &layout_ctx)?;
    let _late_address_relax = crate::codegen::late_address_relax_guard();
    let mut program = crate::codegen::codegen_program_with_async(
        &mwir,
        &flow,
        &layout_ctx,
        &method_index,
        group_arena_capacity,
        &enqueue_specs,
    )
    .map_err(|e| e.message)?;
    if let Some(skeleton) = &pixels_skeleton {
        crate::codegen::install_pixels_plane_renderer(
            &mut program,
            &skeleton.frame_program,
            &skeleton.semantic_seed,
        )?;
        crate::cost::audit::audit_program(&program)?;
    }
    let async_frames =
        crate::codegen::async_frame_sizes(&flow, &layout_ctx).map_err(|e| e.message)?;
    let (group_child_index, _) =
        crate::codegen::compute_group_child_indices(&flow).map_err(|e| e.message)?;
    Ok(ImageCodegen {
        program,
        modules: live_modules,
        programs: live_programs,
        flow,
        async_frames,
        group_child_index,
        layout_ctx,
        rtconfig_text,
    })
}

fn generate_live_rtconfig(
    wiring: Option<&RuntimeWiring>,
    tests: &[crate::rtconfig::TestRunnerFact],
) -> Result<(String, crate::lower::ImageForceRootOpts), String> {
    match wiring {
        Some(w) => {
            let mut extras = crate::rtconfig::extras_from_tables(&w.tables)?;
            extras.tests = tests.to_vec();
            extras.has_boot_init = true;
            let text = crate::rtconfig::generate_with(&w.tables, &extras)?;
            let opts = crate::lower::ImageForceRootOpts {
                with_wiring: true,
                with_test_runner: !tests.is_empty(),
                n_tests: tests.len(),
                n_boot_calls: w.tables.n_boot_calls,
                n_irq_calls: w.tables.irq_vector_bits.len(),
                n_wake_calls: w.tables.wake_pending_addrs.len(),
                n_cores: w.tables.cores,
            };
            Ok((text, opts))
        }
        None => {
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
            let mut extras = crate::rtconfig::RtconfigExtras::default();
            extras.tests = tests.to_vec();
            extras.has_boot_init = false;
            let text = crate::rtconfig::generate_with(&tables, &extras)?;
            let opts = crate::lower::ImageForceRootOpts {
                with_wiring: false,
                with_test_runner: !tests.is_empty(),
                n_tests: tests.len(),
                n_boot_calls: 0,
                n_irq_calls: 0,
                n_wake_calls: 0,
                n_cores: 1,
            };
            Ok((text, opts))
        }
    }
}

fn recheck_with_live_rtconfig(
    modules: &BTreeMap<String, Module>,
    rtconfig_text: &str,
) -> Result<(BTreeMap<String, Module>, BTreeMap<String, TypedProgram>), String> {
    let gen_module = crate::rtconfig::parse_generated(rtconfig_text)?;
    let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let mut modules_vec: BTreeMap<Vec<String>, Module> = BTreeMap::new();
    let mut paths: BTreeMap<Vec<String>, String> = BTreeMap::new();
    for (dot, m) in modules {
        let key: Vec<String> = dot.split('.').map(|s| s.to_string()).collect();
        if key.as_slice() == crate::loader::IMAGE_RUNTIME_MODULE_KEY {
            continue;
        }
        paths.insert(key.clone(), format!("<{dot}>"));
        modules_vec.insert(key, m.clone());
    }
    let (runtime_key, runtime_loaded) = crate::loader::load_runtime_module()
        .map_err(|_| "stdlib/core/runtime.wr missing".to_string())?;
    modules_vec.insert(runtime_key.clone(), runtime_loaded.module);
    paths.insert(runtime_key, runtime_loaded.file.display().to_string());
    modules_vec.insert(gen_key.clone(), gen_module);
    paths.insert(gen_key, crate::rtconfig::GENERATED_INPUT_PATH.to_string());
    let time_key: Vec<String> = crate::loader::TIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let needs_time = modules_vec
        .values()
        .any(crate::loader::module_mentions_time);
    if needs_time && !modules_vec.contains_key(&time_key) {
        let (loaded_key, time_loaded) = crate::loader::load_time_module()
            .map_err(|_| "stdlib/core/time.wr missing".to_string())?;
        debug_assert_eq!(loaded_key, time_key);
        paths.insert(time_key.clone(), time_loaded.file.display().to_string());
        modules_vec.insert(time_key, time_loaded.module);
    }
    let programs_vec = crate::sema::check_program_typed(&modules_vec, &paths).map_err(|e| {
        format!(
            "error[{}]: live rtconfig re-check: {}",
            e.category, e.message
        )
    })?;
    let programs: BTreeMap<String, TypedProgram> = programs_vec
        .into_iter()
        .map(|(k, p)| (k.join("."), p))
        .collect();
    let modules: BTreeMap<String, Module> = modules_vec
        .into_iter()
        .map(|(k, m)| (k.join("."), m))
        .collect();
    Ok((modules, programs))
}

fn lower_mwir_closure(
    programs: &BTreeMap<String, TypedProgram>,
    capacity: Option<u64>,
    opts: &crate::lower::LowerOpts,
) -> Result<mwir::MwirProgram, String> {
    let mut mwir_programs = Vec::with_capacity(programs.len());
    for typed in programs.values() {
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        mwir_programs
            .push(crate::lower::lower_program_with(&stamped, opts).map_err(|e| e.message)?);
    }
    Ok(merge_mwir_programs(mwir_programs))
}

fn lower_flow_closure(
    programs: &BTreeMap<String, TypedProgram>,
    capacity: Option<u64>,
    opts: &crate::lower::LowerOpts,
) -> Result<FlowWirProgram, String> {
    let mut flow_fns = BTreeMap::new();
    for typed in programs.values() {
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        flow_fns.extend(
            crate::flowwir_lower::lower_program_with(&stamped, opts)
                .map_err(|e| e.message)?
                .fns,
        );
    }
    Ok(FlowWirProgram { fns: flow_fns })
}

pub(super) fn apply_resume_remaps(program: &mut CodegenProgram, wiring: &RuntimeWiring) {
    let Ok(extras) = crate::rtconfig::extras_from_tables(&wiring.tables) else {
        return;
    };
    let remaps = crate::rtconfig::stub_call_remaps(&extras, wiring.tables.cores);
    if remaps.is_empty() {
        return;
    }
    for f in program.fns.values_mut() {
        crate::rtconfig::remap_call_keys(f, &remaps);
    }
}

pub fn try_layout_with_codegen(
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
) -> Result<Option<(ImageLayout, CodegenProgram)>, String> {
    let empty_tests: &[String] = &[];
    let empty_async = BTreeSet::new();
    {
        let inits = actor_inits(modules).map_err(|e| e.message)?;
        let layouts = closure_layout_types(modules, programs).map_err(|e| e.message)?;
        let backings =
            crate::eval::image_checks::pool_backings(graph, &layouts).map_err(|e| e.message)?;
        build_boot_init_calls(graph, &inits, &backings).map_err(|e| e.message)?;
    }
    let compiled = match lower_and_codegen_image(
        modules,
        programs,
        layout_ctx,
        graph,
        empty_tests,
        &empty_async,
        false,
    ) {
        Ok(c) => c,
        Err(e) if e.starts_with(crate::codegen::FAIL_CLOSED_PREFIX) => return Err(e),
        Err(_) => return Ok(None),
    };
    layout_program(
        &compiled.program,
        Some(BootCtx {
            graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
            layout_ctx: &compiled.layout_ctx,
            async_frames: &compiled.async_frames,
            group_child_index: &compiled.group_child_index,
            flow: &compiled.flow,
        }),
    )
    .and_then(|mut layout| {
        attach_blk_report(&mut layout, graph, &compiled.programs)?;
        layout.placed_statics = collect_placed_statics(&compiled.programs)?;
        Ok(Some((layout, compiled.program)))
    })
    .or_else(|e| Err(e.message))
}

pub fn try_layout_program(
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
) -> Result<Option<ImageLayout>, String> {
    Ok(try_layout_with_codegen(programs, layout_ctx, graph, modules)?.map(|(layout, _)| layout))
}

fn collect_placed_statics(
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<Vec<PlacedStatic>, LayoutError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for prog in programs.values() {
        for (name, s) in &prog.statics {
            if !seen.insert(name.clone()) {
                continue;
            }
            let crate::sema::types::Type::Named(ty_name, _) = &s.ty else {
                return Err(LayoutError::new(format!(
                    "internal error: placed static `{name}` has non-named type"
                )));
            };
            let size = prog
                .layouts
                .iter()
                .find(|l| l.name == *ty_name)
                .and_then(|l| l.size)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: placed static `{name}` type `{ty_name}` has no completed size"
                    ))
                })?;
            out.push(PlacedStatic {
                name: name.clone(),
                ty: ty_name.clone(),
                addr: s.addr,
                size,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    verify_device_window_statics(&out)?;
    Ok(out)
}

const DEVICE_WINDOW_LO: u64 = 0x4000_8000;
const DEVICE_WINDOW_HI: u64 = 0x4001_0000;

fn verify_device_window_statics(placed: &[PlacedStatic]) -> Result<(), LayoutError> {
    let mut in_window: Vec<&PlacedStatic> = placed
        .iter()
        .filter(|p| p.addr >= DEVICE_WINDOW_LO && p.addr < DEVICE_WINDOW_HI)
        .collect();
    in_window.sort_by_key(|p| p.addr);
    for p in &in_window {
        let end = p.addr.saturating_add(p.size);
        if end > DEVICE_WINDOW_HI {
            return Err(LayoutError::new(format!(
                "placed static `{}` (`{}`) spans {:#x}..{:#x}, past the end of the reserved \
                 device-page window ({DEVICE_WINDOW_LO:#x}..{DEVICE_WINDOW_HI:#x}) — this image \
                 declares too many cores for the per-core `LANE1` stripe \
                 (plans/lane1-per-core.md item A: one {}-byte row per core); place fewer cores",
                p.name, p.ty, p.addr, end, p.size
            )));
        }
    }
    for pair in in_window.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let a_end = a.addr.saturating_add(a.size);
        if a_end > b.addr {
            return Err(LayoutError::new(format!(
                "placed statics `{}` (`{}`, {:#x}..{:#x}) and `{}` (`{}`, at {:#x}) overlap in the \
                 reserved device-page window ({DEVICE_WINDOW_LO:#x}..{DEVICE_WINDOW_HI:#x}) — a \
                 per-core stripe grew into the next page; place fewer cores",
                a.name, a.ty, a.addr, a_end, b.name, b.ty, b.addr
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::boot_init::{
        ActorInit, HandleSpace, actor_inits, boot_init_arg_word, image_decl_handle_word,
    };
    use super::*;
    use crate::codegen::CodegenFn;

    fn fn_words(words: &[u32]) -> CodegenFn {
        CodegenFn {
            frame_size: 16,
            code: words
                .iter()
                .map(|w| {
                    crate::cost::EmittedWord::new(
                        *w,
                        String::new(),
                        crate::cost::CostRule::Alu,
                        None,
                        &[],
                    )
                })
                .collect(),
            relocs: Vec::new(),
        }
    }

    #[test]
    fn closure_layout_types_completes_runtime_const_lengths() {
        let src = "\
module examples.e1_closure_layout_complete

const N_TURNS: u32 = 4

@layout(runtime, endian=little)
struct TurnArea:
    state: u32
    waiter: u32

@layout(runtime, endian=little)
struct TurnTable:
    rr_cursor: u64
    turns: [TurnArea; N_TURNS]
";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let key = module.path.join(".");
        let program = crate::sema::check_typed(&module, "<e1>").expect("check");
        let mut modules = BTreeMap::new();
        modules.insert(key.clone(), module);
        let mut programs = BTreeMap::new();
        programs.insert(key, program);

        let layouts = closure_layout_types(&modules, &programs)
            .expect("closure_layout_types completes rather than rejecting");
        let table = layouts
            .get("TurnTable")
            .expect("TurnTable is in the closure");
        assert_eq!(
            table
                .require_size("closure_layout_types after E1")
                .expect("completed"),
            40
        );
        assert_eq!(table.size, Some(40));
    }

    #[test]
    fn force_rooted_probe_resolves_via_bl_call_key() {
        let src = "\
module examples.bl_call_probe

@test(runtime)
pub fn t():
    return
";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("runtime.wr must load"),
        };
        let root_key = module.path.clone();
        let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let gen_module = crate::rtconfig::parse_generated(&crate::rtconfig::stub_text())
            .expect("stub must parse");
        let mut modules_vec = BTreeMap::new();
        modules_vec.insert(root_key.clone(), module.clone());
        modules_vec.insert(runtime_key.clone(), runtime_loaded.module.clone());
        modules_vec.insert(gen_key.clone(), gen_module);
        let mut paths = BTreeMap::new();
        paths.insert(root_key.clone(), "<test>".to_string());
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        paths.insert(gen_key, crate::rtconfig::GENERATED_INPUT_PATH.to_string());
        let programs_vec = crate::sema::check_program_typed(&modules_vec, &paths).expect("check");
        let programs: BTreeMap<String, crate::sema::typed::TypedProgram> = programs_vec
            .into_iter()
            .map(|(k, p)| (k.join("."), p))
            .collect();
        let modules: BTreeMap<String, Module> = modules_vec
            .into_iter()
            .map(|(k, m)| (k.join("."), m))
            .collect();
        let reachable = crate::lower::guest_reachable_keys_closure(
            &programs,
            &crate::lower::LowerOpts::default(),
        );
        assert!(reachable.contains("__wrela_runtime_probe"));
        let lower_opts = crate::lower::LowerOpts {
            emit_comptime_tests: false,
            only: Some(reachable),
        };
        let mut mwir_programs = Vec::new();
        let mut flow_fns = BTreeMap::new();
        for typed in programs.values() {
            mwir_programs.push(crate::lower::lower_program_with(typed, &lower_opts).expect("mwir"));
            flow_fns.extend(
                crate::flowwir_lower::lower_program_with(typed, &lower_opts)
                    .expect("flow")
                    .fns,
            );
        }
        let mwir = merge_mwir_programs(mwir_programs);
        let flow = crate::flowwir::FlowWirProgram { fns: flow_fns };
        let mut layout_ctx = merge_layout_ctx(&modules).expect("layout ctx");
        enrich_layout_ctx_with_instantiations(&mut layout_ctx, &programs);
        let method_index = actor_method_index_tables(&modules, &layout_ctx).expect("index");
        let codegen = crate::codegen::codegen_program_with_async(
            &mwir,
            &flow,
            &layout_ctx,
            &method_index,
            0,
            &[],
        )
        .expect("codegen");
        assert!(
            codegen.fns.contains_key("__wrela_runtime_probe"),
            "probe must be in codegen fns"
        );
        let mut a = Asm::new(0);
        a.bl_call_key("__wrela_runtime_probe");
        a.push(encode::enc_ret(30));
        let mut fns = codegen.fns.clone();
        fns.insert(
            "__wrela_hand_asm_caller".into(),
            crate::codegen::CodegenFn {
                frame_size: 16,
                code: a
                    .words
                    .iter()
                    .map(|w| {
                        crate::cost::EmittedWord::new(
                            *w,
                            String::new(),
                            crate::cost::CostRule::Alu,
                            None,
                            &[],
                        )
                    })
                    .collect(),
                relocs: a.relocs,
            },
        );
        let codegen = crate::codegen::CodegenProgram {
            fns,
            rodata: codegen.rodata.clone(),
            ..Default::default()
        };
        let laid = layout_program(&codegen, None)
            .expect("layout must resolve bl_call_key to force-rooted probe");
        assert!(!laid.sections.is_empty());
    }

    #[test]
    fn checkpoint_section_ignores_irq_wake_lists() {
        let linked = build_checkpoint_and_vector_stub(None);
        let empty = build_checkpoint_and_vector_stub_ex(None, &[], &[], true);
        assert_eq!(
            linked.words, empty.words,
            "empty irq/wake must match the linked trampoline"
        );
        assert_eq!(linked.relocs, empty.relocs);
        let multi = build_checkpoint_and_vector_stub_ex(
            None,
            &[IrqVectorEntry {
                vector: 1,
                handler_key: "struct:BlkDriver.on_queue_irq".into(),
                driver_state: 0x4050_0000,
            }],
            &[WakeDrainEntry {
                driver_state: 0x4050_0000,
                wake_drain_index: 0,
                task_key: "struct:BlkDriver.drain".into(),
            }],
            true,
        );
        assert_eq!(
            multi.words, linked.words,
            "irq/wake must not change checkpoint section words after item I"
        );
        assert_eq!(
            linked.words.len(),
            7,
            "floor 5 + mov x0,#0 + BL (M13 item N per-core arg)"
        );
        assert_eq!(linked.relocs.len(), 1, "one BL to __wrela_rt_checkpoint");
        let bare = build_checkpoint_and_vector_stub_ex(None, &[], &[], false);
        assert_eq!(bare.words.len(), 1, "unlinked section is bare ret");
    }

    fn parse_one_module(src: &str) -> Module {
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        crate::syntax::parser::parse(tokens).expect("parse")
    }

    fn one_module(name: &str, src: &str) -> BTreeMap<String, Module> {
        let mut m = BTreeMap::new();
        m.insert(name.to_string(), parse_one_module(src));
        m
    }

    #[test]
    fn merge_layout_ctx_keys_aliased_imports_under_local_spelling() {
        let mut modules = BTreeMap::new();
        modules.insert(
            "lib.pair".to_string(),
            parse_one_module(
                "\
module lib.pair

pub struct Pair:
    a: u32
    b: u32

pub enum Color:
    Red
    Blue
",
            ),
        );
        modules.insert(
            "app.main".to_string(),
            parse_one_module(
                "\
module app.main

from lib.pair import Pair as Duo
from lib.pair import Color as Hue

fn use(d: Duo, h: Hue):
    pass
",
            ),
        );
        let ctx = merge_layout_ctx(&modules).unwrap();
        assert!(
            ctx.structs.contains_key("Duo"),
            "aliased struct must be keyed under the local spelling"
        );
        assert!(
            ctx.structs.contains_key("Pair"),
            "exporter's own spelling remains for the exporter module"
        );
        assert!(
            ctx.enums.contains_key("Hue"),
            "aliased enum must be keyed under the local spelling"
        );
        assert!(ctx.enums.contains_key("Color"));
        assert_eq!(
            ctx.struct_field_names.get("Duo").map(|v| v.as_slice()),
            Some(["a".to_string(), "b".to_string()].as_slice())
        );
        let duo = crate::sema::types::Type::Named("Duo".to_string(), vec![]);
        assert_eq!(mwir::size_of(&duo, &ctx), Ok(16));
        let hue = crate::sema::types::Type::Named("Hue".to_string(), vec![]);
        assert_eq!(mwir::size_of(&hue, &ctx), Ok(8));
    }

    fn actor_decl(actor_type: &str, mailbox: Option<u32>) -> crate::eval::image::ActorDecl {
        use crate::eval::image::DeclArg;
        use crate::eval::value::Value;
        use crate::sema::types::Type;
        let mut args = Vec::new();
        if let Some(n) = mailbox {
            args.push(DeclArg {
                label: "mailbox".to_string(),
                ty: Type::U32,
                value: Value::U32(n),
            });
        }
        crate::eval::image::ActorDecl {
            actor_type: Type::Named(actor_type.to_string(), vec![]),
            args,
        }
    }

    #[test]
    fn compute_runtime_tables_is_none_for_a_sync_only_image() {
        let modules = one_module("m", "module m\n\nfn f():\n    pass\n");
        let graph = ImageGraph::default();
        let ctx = merge_layout_ctx(&modules).unwrap();
        let out = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
        .unwrap();
        assert!(
            out.is_none(),
            "no actors and no async fns -> no runtime tables at all"
        );
    }

    #[test]
    fn compute_runtime_tables_sizes_state_mailbox_and_slot() {
        let src = "\
module m

@actor
pub struct Store:
    count: u32
    total: u64

    init(mut self):
        self.count = 0
        self.total = 0

    pub fn get(read self) -> u64:
        return self.total

    pub fn bump(mut self, by: u32) -> u64:
        self.total = self.total + by.to[u64]()
        return self.total
";
        let modules = one_module("m", src);
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(4)));
        let ctx = merge_layout_ctx(&modules).unwrap();
        let tables = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
        .unwrap()
        .expect("one actor -> Some");
        assert_eq!(tables.actors.len(), 1);
        let a = &tables.actors[0];
        assert_eq!(a.name, "Store");
        assert_eq!(a.mailbox_capacity, 4);
        assert_eq!(a.state_size, 16);
        assert_eq!(a.slot_size, 24);
        assert_eq!(a.frame_size, crate::codegen::TURN_RECORD_SIZE);
        assert_eq!(tables.ready_queue_capacity, 2);
        assert_eq!(tables.group_arena_capacity, 0);
        assert_eq!(tables.n_turns, 1);
        assert_eq!(tables.turn_stride, 64);
        let expect_total = a.state_size
            + a.mailbox_capacity as u64 * a.slot_size
            + 24
            + tables.n_turns * tables.turn_stride
            + tables.ready_queue_capacity * 8
            + 8;
        assert_eq!(tables.total_bytes, expect_total);
    }

    #[test]
    fn group_service_ctx_includes_messageable_driver_turns() {
        let tables = RuntimeTables {
            actors: vec![ActorRuntimeLayout {
                name: "A".to_string(),
                state_size: 8,
                mailbox_capacity: 2,
                slot_size: 16,
                frame_size: 64,
            }],
            drivers: vec![
                DriverRuntimeLayout {
                    name: "Msg".to_string(),
                    state_size: 8,
                    has_wake: false,
                    wake_drain_index: None,
                    mailbox: Some(DriverMailbox {
                        capacity: 2,
                        slot_size: 16,
                        frame_size: 64,
                    }),
                },
                DriverRuntimeLayout {
                    name: "Silent".to_string(),
                    state_size: 8,
                    has_wake: false,
                    wake_drain_index: None,
                    mailbox: None,
                },
            ],
            free_turns: vec![("f".to_string(), 64)],
            n_turns: 3,
            turn_stride: 64,
            group_arena_capacity: 1,
            ready_queue_capacity: 3,
            ..RuntimeTables::default()
        };
        let base = 0x4000u64;
        let p = place_runtime_tables(base, &tables);
        let ctx = group_service_ctx(&p, &tables).expect("group arena present");
        assert_eq!(
            ctx.turn_areas.len(),
            3,
            "actors + messageable drivers + free turns"
        );
        assert_eq!(ctx.turn_areas[0].0, p.actors[0].turn);
        assert_eq!(ctx.turn_areas[0].1, TurnId::from_index(0));
        let driver = p
            .driver_mailboxes
            .get(&0)
            .expect("messageable driver placed");
        assert_eq!(ctx.turn_areas[1].0, driver.turn);
        assert_eq!(ctx.turn_areas[1].1, TurnId::from_index(1));
        assert_eq!(ctx.turn_areas[2].0, p.free_turns["f"]);
        assert_eq!(ctx.turn_areas[2].1, TurnId::from_index(2));
        let shape = group_service_shape(Some(&tables)).expect("shape");
        assert_eq!(
            shape.turn_areas.len(),
            ctx.turn_areas.len(),
            "shape length must match real ctx (word-count contract)"
        );
    }

    #[test]
    fn place_runtime_tables_groups_every_turn_into_one_array_at_the_base() {
        let tables = RuntimeTables {
            actors: vec![
                ActorRuntimeLayout {
                    name: "A".to_string(),
                    state_size: 8,
                    mailbox_capacity: 2,
                    slot_size: 16,
                    frame_size: 64,
                },
                ActorRuntimeLayout {
                    name: "B".to_string(),
                    state_size: 8,
                    mailbox_capacity: 2,
                    slot_size: 16,
                    frame_size: 120,
                },
            ],
            free_turns: vec![("f".to_string(), 64)],
            n_turns: 3,
            turn_stride: 128,
            ready_queue_capacity: 3,
            ..RuntimeTables::default()
        };
        let base = 0x4000u64;
        let p = place_runtime_tables(base, &tables);

        assert_eq!(p.turns_base, base, "`turns_base` is `rtdata_base` itself");
        assert_eq!(p.turn_stride, tables.turn_stride);
        assert_eq!(p.actors[0].turn, base);
        assert_eq!(p.actors[1].turn, base + 128);
        assert_eq!(p.free_turns["f"], base + 256);

        assert_eq!(p.actors[0].state, base + 3 * 128);
        assert_eq!(p.actors[1].state, p.actors[0].count + 8);

        assert_eq!(p.turn_ids["f"].get(), 3);
        assert_eq!(p.turn_ids["f"].index(), 2);
        assert_eq!(TurnId::from_index(0).get(), 1);
        for (key, want) in [("A.tick", base), ("B.tick", base + 128), ("f", base + 256)] {
            let id = p.turn_id_for(key, &tables).expect("a sized turn owner");
            assert_eq!(p.turn_addr(id), want, "{key}");
            assert_eq!(p.turn_area_for(key, &tables), Some(want), "{key}");
        }
    }

    #[test]
    fn compute_runtime_tables_fails_closed_without_a_declared_mailbox() {
        let modules = one_module(
            "m",
            "module m\n\n@actor\npub struct Store:\n    count: u32\n\n    init(mut self):\n        self.count = 0\n",
        );
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", None));
        let ctx = merge_layout_ctx(&modules).unwrap();
        let err = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
        .unwrap_err();
        assert!(err.contains("mailbox"));
    }

    #[test]
    fn private_methods_never_contribute_to_the_message_slot_size() {
        let src = "\
module m

@actor
pub struct Store:
    count: u64

    init(mut self):
        self.count = 0

    pub fn get(read self) -> u64:
        return self.count

    fn helper(read self, huge: u64, more: u64, extra: u64) -> u64:
        return huge + more + extra
";
        let modules = one_module("m", src);
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(2)));
        let ctx = merge_layout_ctx(&modules).unwrap();
        let tables = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
        .unwrap()
        .unwrap();
        assert_eq!(tables.actors[0].slot_size, 16);
    }

    fn wired(
        label: &str,
        ty: crate::sema::types::Type,
        value: crate::eval::value::Value,
    ) -> crate::eval::image::DeclArg {
        crate::eval::image::DeclArg {
            label: label.to_string(),
            ty,
            value,
        }
    }

    fn decl_with(
        actor_type: &str,
        args: Vec<crate::eval::image::DeclArg>,
    ) -> crate::eval::image::ActorDecl {
        crate::eval::image::ActorDecl {
            actor_type: crate::sema::types::Type::Named(actor_type.to_string(), vec![]),
            args,
        }
    }

    #[test]
    fn an_integer_init_argument_is_its_own_sign_extended_word() {
        use crate::eval::value::Value;
        let z = HandleSpace::default();
        assert_eq!(boot_init_arg_word(&Value::U8(200), z), Some(200));
        assert_eq!(boot_init_arg_word(&Value::U16(40000), z), Some(40000));
        assert_eq!(boot_init_arg_word(&Value::U64(u64::MAX), z), Some(u64::MAX));
        assert_eq!(
            boot_init_arg_word(&Value::I32(-3), z),
            Some(0xFFFF_FFFF_FFFF_FFFD)
        );
        assert_eq!(boot_init_arg_word(&Value::I64(-1), z), Some(u64::MAX));
        assert_eq!(boot_init_arg_word(&Value::Bool(true), z), Some(1));
        assert_eq!(boot_init_arg_word(&Value::Bool(false), z), Some(0));
        assert_eq!(boot_init_arg_word(&Value::Char('A'), z), Some(65));
        assert_eq!(boot_init_arg_word(&Value::Unit, z), Some(0));
    }

    #[test]
    fn a_handle_init_argument_is_its_own_construction_order_index() {
        use crate::eval::image::ImageDeclRef;
        use crate::eval::value::Value;
        let space = HandleSpace {
            n_actors: 3,
            n_drivers: 2,
        };
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Actor(2)), space),
            Some(2)
        );
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Driver(1)), space),
            Some(4)
        );
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Device(0)), space),
            Some(5)
        );
        assert_eq!(
            boot_init_arg_word(
                &Value::ImageDecl(ImageDeclRef::Pool("Buffers".to_string())),
                HandleSpace::default()
            ),
            None
        );
    }

    #[test]
    fn actor_and_driver_handle_words_never_collide() {
        use crate::eval::image::ImageDeclRef;
        use crate::eval::value::Value;
        let space = HandleSpace {
            n_actors: 1,
            n_drivers: 1,
        };
        let actor0 = boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Actor(0)), space);
        let driver0 = boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Driver(0)), space);
        assert_eq!(actor0, Some(0));
        assert_eq!(driver0, Some(1));
        assert_ne!(actor0, driver0);
    }

    #[test]
    fn image_decl_handle_words_are_duplicate_free() {
        use crate::eval::image::ImageDeclRef;
        use std::collections::BTreeSet;
        let space = HandleSpace {
            n_actors: 2,
            n_drivers: 1,
        };
        let decls = [
            ImageDeclRef::Actor(0),
            ImageDeclRef::Actor(1),
            ImageDeclRef::Driver(0),
            ImageDeclRef::Device(0),
            ImageDeclRef::Device(1),
        ];
        let mut words = BTreeSet::new();
        for d in &decls {
            let w = image_decl_handle_word(space, d)
                .unwrap_or_else(|| panic!("indexed kind {d:?} must have a handle word"));
            assert!(
                words.insert(w),
                "duplicate handle word {w} for {d:?} — image_decl_handle_word's \
                 contract is that no two distinct declarations share a word"
            );
        }
        assert_eq!(words.len(), decls.len());
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Pool("Buffers".into())),
            None
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::DmaPool("Payloads".into())),
            None
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Actor(0)),
            Some(0)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Actor(1)),
            Some(1)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Driver(0)),
            Some(2)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Device(0)),
            Some(3)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Device(1)),
            Some(4)
        );
    }

    #[test]
    fn resolve_runtime_test_args_uses_the_shared_handle_space() {
        let mut graph = ImageGraph::default();
        graph.actors.push(crate::eval::image::ActorDecl {
            actor_type: crate::sema::types::Type::Named("Scale".into(), vec![]),
            args: vec![],
        });
        graph.drivers.push(crate::eval::image::DriverDecl {
            actor_type: crate::sema::types::Type::Named("BlkDriver".into(), vec![]),
            args: vec![crate::eval::image::DeclArg {
                label: "mailbox".into(),
                ty: crate::sema::types::Type::U64,
                value: crate::eval::value::Value::U64(4),
            }],
        });
        let src = "\
module m

@actor
pub struct Scale:
    pub fn get(read self) -> u64:
        return 1

@driver
pub struct BlkDriver:
    pub fn get(read self) -> u64:
        return 2

@test(runtime)
async fn asks_actor(s: Actor[Scale]):
    v = await s.get()
    @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, \"rejected\"

@test(runtime)
async fn asks_driver(d: Actor[BlkDriver]):
    v = await d.get()
    @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, \"rejected\"
";
        let modules = one_module("m", src);
        let program = crate::sema::check_typed(modules.get("m").unwrap(), "m.wr").expect("types");
        let names = vec!["asks_actor".into(), "asks_driver".into()];
        let args = resolve_runtime_test_args(&program, &names, &graph).expect("resolve");
        assert_eq!(
            args.get("asks_actor").map(|v| v.as_slice()),
            Some([0u64].as_slice())
        );
        assert_eq!(
            args.get("asks_driver").map(|v| v.as_slice()),
            Some([1u64].as_slice())
        );
    }

    #[test]
    fn an_aggregate_or_float_init_argument_has_no_word_at_all() {
        use crate::eval::value::Value;
        let z = HandleSpace::default();
        assert_eq!(boot_init_arg_word(&Value::F64(1.0), z), None);
        assert_eq!(boot_init_arg_word(&Value::Str(b"hi".to_vec()), z), None);
        assert_eq!(
            boot_init_arg_word(&Value::Tuple(vec![Value::U8(1)]), z),
            None
        );
        assert_eq!(
            boot_init_arg_word(&Value::Array(vec![Value::U8(1)]), z),
            None
        );
        assert_eq!(
            boot_init_arg_word(&Value::Struct(vec![Value::U8(1)]), z),
            None
        );
        assert_eq!(boot_init_arg_word(&Value::Enum(0, vec![]), z), None);
    }

    #[test]
    fn init_arguments_are_ordered_by_the_declared_parameter_list_not_the_wiring() {
        use crate::eval::value::Value;
        use crate::sema::types::Type;
        let src = "\
module m

@actor
pub struct Store:
    lo: u8
    hi: u16

    init(mut self, lo: u8, hi: u16):
        self.lo = lo
        self.hi = hi
";
        let modules = one_module("m", src);
        let inits = actor_inits(&modules).unwrap();
        let mut graph = ImageGraph::default();
        graph.actors.push(decl_with(
            "Store",
            vec![
                wired("hi", Type::U16, Value::U16(40000)),
                wired("mailbox", Type::U32, Value::U32(4)),
                wired("lo", Type::U8, Value::U8(200)),
            ],
        ));
        let (calls, _) = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap();
        let call = calls[0].as_ref().expect("Store declares an `init`");
        assert_eq!(call.key, "Store.init");
        assert_eq!(
            call.args,
            vec![BootInitArg::Word(200), BootInitArg::Word(40000)]
        );
    }

    #[test]
    fn an_actor_with_no_declared_init_gets_no_boot_call() {
        let src = "\
module m

@actor
pub struct Store:
    count: u32
";
        let modules = one_module("m", src);
        let inits = actor_inits(&modules).unwrap();
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(4)));
        let (calls, _) = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap();
        assert!(
            calls[0].is_none(),
            "no `init` means the zero-fill is the whole construction"
        );
    }

    #[test]
    fn a_field_wired_scalar_must_also_equal_the_state_fills_zero() {
        use crate::eval::image::DeclArg;
        use crate::eval::value::Value;
        use crate::sema::types::Type;

        let src = "\
module m

@actor
pub struct Store:
    seed: u32
";
        let modules = one_module("m", src);
        let inits = actor_inits(&modules).unwrap();

        let wired = |v: Value| {
            let mut d = actor_decl("Store", Some(4));
            d.args.push(DeclArg {
                label: "seed".to_string(),
                ty: Type::I64,
                value: v,
            });
            let mut graph = ImageGraph::default();
            graph.actors.push(d);
            build_boot_init_calls(&graph, &inits, &BTreeMap::new())
        };

        assert!(wired(Value::I64(0)).is_ok());

        let err = wired(Value::I64(8)).expect_err("8 is not the state-fill's zero");
        assert!(err.message.contains("materializes as 8"), "{}", err.message);
        assert!(
            err.message.contains("declares no `init`"),
            "{}",
            err.message
        );

        let err = wired(Value::Str(b"x".to_vec())).expect_err("no register representation");
        assert!(
            err.message.contains("no register representation at all"),
            "{}",
            err.message
        );

        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(4)));
        assert!(build_boot_init_calls(&graph, &inits, &BTreeMap::new()).is_ok());
    }

    fn cap_init(cap_ty: crate::sema::types::Type) -> BTreeMap<String, ActorInit> {
        use crate::sema::types::{DeclParam, Type};
        use crate::syntax::ast::AccessMode;
        let mut inits = BTreeMap::new();
        inits.insert(
            "Blk".to_string(),
            ActorInit {
                key: "Blk.init".to_string(),
                params: vec![DeclParam {
                    mode: AccessMode::Take,
                    name: "cap".to_string(),
                    ty: cap_ty,
                }],
                ret: Type::Unit,
            },
        );
        inits
    }

    fn named1(name: &str, arg: &str) -> crate::sema::types::Type {
        use crate::sema::types::{Type, TypeArg};
        Type::Named(
            name.to_string(),
            vec![TypeArg::Type(Type::Named(arg.to_string(), vec![]))],
        )
    }

    #[test]
    fn a_driver_device_cap_materializes_its_devices_register_window() {
        let inits = cap_init(named1("DeviceCap", "VirtioBlock"));
        let mut graph = ImageGraph::default();
        graph.devices.push(crate::eval::image::DeviceDecl {
            device_type: crate::sema::types::Type::Named("VirtioBlock".to_string(), vec![]),
            args: Vec::new(),
        });
        graph.drivers.push(crate::eval::image::DriverDecl {
            actor_type: crate::sema::types::Type::Named("Blk".to_string(), vec![]),
            args: vec![wired(
                "device",
                crate::sema::types::Type::Named("Image".to_string(), vec![]),
                crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Device(0)),
            )],
        });
        let (actors, drivers) = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap();
        assert!(actors.is_empty());
        let call = drivers[0].as_ref().expect("the driver declares an `init`");
        assert_eq!(call.args, vec![BootInitArg::DeviceRegsBase(0)]);
        let regs = vec![DeviceRegs {
            device: 0,
            device_type: "VirtioBlock".to_string(),
            driver: "Blk".to_string(),
            base: 0x4050_1234,
            size: 8,
        }];
        assert_eq!(call.args[0].resolve(&regs, &[]).unwrap(), 0x4050_1234);
        let err = call.args[0].resolve(&[], &[]).unwrap_err();
        assert!(
            err.message.contains("no placed register window"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_device_cap_with_no_device_binding_fails_closed() {
        let inits = cap_init(named1("DeviceCap", "VirtioBlock"));
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Blk", Some(4)));
        let err = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap_err();
        assert!(err.message.contains("binds no device"), "{}", err.message);
    }

    #[test]
    fn every_other_capability_and_every_state_still_fails_closed() {
        for (ty, needle) in [
            (named1("Mmio", "VirtioIrqMmio"), "map_partition"),
            (named1("IrqCap", "V"), "vector="),
            (
                named1("DriverClaimedDevice", "VirtioBlock"),
                "produced by a transition inside the driver",
            ),
        ] {
            let inits = cap_init(ty);
            let mut graph = ImageGraph::default();
            graph.devices.push(crate::eval::image::DeviceDecl {
                device_type: crate::sema::types::Type::Named("VirtioBlock".to_string(), vec![]),
                args: Vec::new(),
            });
            graph.drivers.push(crate::eval::image::DriverDecl {
                actor_type: crate::sema::types::Type::Named("Blk".to_string(), vec![]),
                args: vec![wired(
                    "device",
                    crate::sema::types::Type::Named("Image".to_string(), vec![]),
                    crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Device(
                        0,
                    )),
                )],
            });
            let err = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap_err();
            assert!(err.message.contains(needle), "{}", err.message);
        }
    }

    #[test]
    fn more_than_eight_init_arguments_fails_closed() {
        use crate::eval::value::Value;
        use crate::sema::types::{DeclParam, Type};
        use crate::syntax::ast::AccessMode;
        let params: Vec<DeclParam> = (0..9)
            .map(|i| DeclParam {
                mode: AccessMode::Read,
                name: format!("a{i}"),
                ty: Type::U32,
            })
            .collect();
        let mut inits = BTreeMap::new();
        inits.insert(
            "Wide".to_string(),
            ActorInit {
                key: "Wide.init".to_string(),
                params,
                ret: Type::Unit,
            },
        );
        let args = (0..9)
            .map(|i| wired(&format!("a{i}"), Type::U32, Value::U32(i)))
            .collect();
        let mut graph = ImageGraph::default();
        graph.actors.push(decl_with("Wide", args));
        let err = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap_err();
        assert!(err.message.contains("at most 8"), "{}", err.message);
    }

    #[test]
    fn boot_init_call_stub_emits_init_call_reloc() {
        use crate::codegen::{
            BootInitArgSpec, BootInitCallSpec, BootInitSlotSpec, Reloc, emit_boot_init_call,
        };
        let slot = BootInitSlotSpec {
            name: "A".into(),
            is_driver: false,
            state_size: 8,
            init: Some(BootInitCallSpec {
                key: "A.init".into(),
                args: vec![BootInitArgSpec::Word(7)],
                fallible: false,
                err_msg: None,
            }),
        };
        let f = emit_boot_init_call(&slot);
        assert!(
            f.relocs.iter().any(|r| matches!(
                r,
                Reloc::Call { key, .. } if key == "A.init"
            )),
            "boot init call stub must Call the init key"
        );
    }

    #[test]
    fn count_with_group_sites_counts_every_with_group_and_none_else() {
        let src = "\
module m

async fn one():
    with group(capacity=1) as g:
        pass

fn two():
    if true:
        with group(capacity=2) as g:
            pass
";
        let modules = one_module("m", src);
        assert_eq!(count_with_group_sites(&modules), 2);
    }

    #[test]
    fn count_with_group_sites_is_zero_with_no_with_statements() {
        let modules = one_module("m", "module m\n\nfn f():\n    pass\n");
        assert_eq!(count_with_group_sites(&modules), 0);
    }

    #[test]
    fn patch_bl_encodes_a_forward_offset() {
        let mut words = vec![encode::enc_bl(0)];
        patch_bl(&mut words, 0, 0x1000, 0x1010).unwrap();
        assert_eq!(words[0], encode::enc_bl(0x10));
    }

    #[test]
    fn patch_bl_encodes_a_negative_offset() {
        let mut words = vec![encode::enc_bl(0)];
        patch_bl(&mut words, 0, 0x2000, 0x1000).unwrap();
        assert_eq!(words[0], encode::enc_bl(-0x1000));
    }

    #[test]
    fn patch_bl_keeps_a_tail_calls_own_b_form() {
        let mut words = vec![encode::enc_b(0)];
        patch_bl(&mut words, 0, 0x1000, 0x1010).unwrap();
        assert_eq!(words[0], encode::enc_b(0x10));
        assert_ne!(words[0], encode::enc_bl(0x10));
    }

    #[test]
    fn patch_bl_fails_closed_on_a_word_that_is_not_a_branch() {
        let mut words = vec![0u32];
        assert!(patch_bl(&mut words, 0, 0x1000, 0x1010).is_err());
    }

    #[test]
    fn patch_bl_fails_closed_out_of_range() {
        let mut words = vec![encode::enc_bl(0)];
        let far = 1u64 << 40;
        assert!(patch_bl(&mut words, 0, 0, far).is_err());
    }

    #[test]
    fn patch_adrp_add_same_page() {
        let mut words = vec![encode::enc_adrp(9, 0), encode::enc_add_imm(9, 9, 0, true)];
        patch_adrp_add(&mut words, 0, 0x4050_0004, 0x4050_0ABC).unwrap();
        assert_eq!(words[0], encode::enc_adrp(9, 0));
        assert_eq!(words[1], encode::enc_add_imm(9, 9, 0x0ABC, true));
    }

    #[test]
    fn patch_adrp_add_crossing_pages_backward() {
        let mut words = vec![
            encode::enc_adrp(10, 0),
            encode::enc_add_imm(10, 10, 0, true),
        ];
        patch_adrp_add(&mut words, 0, 0x4050_1000, 0x4050_0040).unwrap();
        assert_eq!(words[0], encode::enc_adrp(10, -1));
        assert_eq!(words[1], encode::enc_add_imm(10, 10, 0x040, true));
    }

    #[test]
    fn patch_adr_resolves_a_byte_distance_in_both_directions() {
        let mut words = vec![encode::enc_adr(9, 0)];
        patch_adr(&mut words, 0, 0x4050_0004, 0x4050_0ABC).unwrap();
        assert_eq!(words[0], encode::enc_adr(9, 0x0ABC - 0x0004));

        let mut words = vec![encode::enc_adr(10, 0)];
        patch_adr(&mut words, 0, 0x4050_1000, 0x4050_0040).unwrap();
        assert_eq!(words[0], encode::enc_adr(10, 0x0040 - 0x1000));
    }

    #[test]
    fn patch_adr_out_of_range_fails_the_build_rather_than_emitting_a_wrong_adr() {
        let mut words = vec![encode::enc_adr(9, 0)];
        let this = 0x4050_0000u64;
        let far = this + ADR_HALF_RANGE_BYTES as u64;
        let err = patch_adr(&mut words, 0, this, far).expect_err("must refuse");
        assert!(
            err.message.contains("relocation out of range")
                && err.message.contains("`ADR` at 0x40500000")
                && err.message.contains("1048576 bytes away")
                && err.message.contains("±1 MiB")
                && err.message.contains("OptId::AdrAddressing"),
            "the refusal must name the site, the distance and the way out: {}",
            err.message
        );
        assert_eq!(
            words[0],
            encode::enc_adr(9, 0),
            "the placeholder must be left untouched on refusal"
        );

        let mut words = vec![encode::enc_adr(9, 0)];
        let this = 0x4050_0000u64;
        let back = this - ADR_HALF_RANGE_BYTES as u64 - 4;
        patch_adr(&mut words, 0, this, back).expect_err("must refuse backward too");

        let mut words = vec![encode::enc_adr(9, 0)];
        patch_adr(&mut words, 0, this, this + ADR_HALF_RANGE_BYTES as u64 - 4).unwrap();
        let mut words = vec![encode::enc_adr(9, 0)];
        patch_adr(&mut words, 0, this, this - ADR_HALF_RANGE_BYTES as u64).unwrap();
    }

    #[test]
    fn late_adr_growth_rewrites_the_reloc_and_shifts_following_words() {
        let first = crate::cost::EmittedWord::new(
            encode::enc_adr(9, 0),
            "adr x9, rodata+0x0".into(),
            crate::cost::CostRule::Adrp,
            Some(9),
            &[],
        );
        let second = crate::cost::EmittedWord::new(
            encode::enc_movz(10, 1, 0, true),
            "movz x10".into(),
            crate::cost::CostRule::MovWide,
            Some(10),
            &[],
        );
        let second_word = second.word;
        let program = CodegenProgram {
            fns: BTreeMap::from([(
                "f".into(),
                crate::codegen::CodegenFn {
                    frame_size: 0,
                    code: vec![first, second],
                    relocs: vec![Reloc::RodataAdr {
                        word: 0,
                        byte_offset: 0,
                    }],
                },
            )]),
            ..CodegenProgram::default()
        };
        let sites = BTreeMap::from([("f".into(), BTreeSet::from([0usize]))]);
        let expanded = expand_rodata_adr_sites(&program, &sites).expect("grow");
        assert_eq!(expanded.fns["f"].code.len(), 3);
        assert!(matches!(
            expanded.fns["f"].relocs.as_slice(),
            [Reloc::Rodata {
                word_adrp: 0,
                byte_offset: 0
            }]
        ));
        assert_eq!(expanded.fns["f"].code[2].word, second_word);
    }

    #[test]
    fn an_adr_addressed_image_lays_out_and_every_site_resolves_to_its_rodata_byte() {
        use crate::opts::{CompileMode, apply_mode};

        apply_mode(CompileMode::Release);
        let src = "module examples.layout_adr_rodata\n\npub fn add(a: u8, b: u8) -> u8:\n    \
                   return a + b\n";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let typed = crate::sema::check_typed(&module, "<test>").expect("check");
        let lctx = crate::mwir::build_layout_ctx(&module, &Default::default()).expect("layout ctx");
        let mwir = crate::lower::lower_program(&typed).expect("lower");
        let program = crate::codegen::codegen_program(&mwir, &lctx).expect("codegen");

        let sites: usize = program
            .fns
            .values()
            .map(|f| {
                f.relocs
                    .iter()
                    .filter(|r| matches!(r, Reloc::RodataAdr { .. }))
                    .count()
            })
            .sum();
        assert!(sites > 0, "this fixture must exercise the new reloc class");

        let out = layout_program(&program, None).expect("an ADR-addressed image must lay out");
        let rodata = out
            .sections
            .iter()
            .find(|s| s.name == "rodata")
            .expect("rodata section");
        let rodata_end = rodata.base + rodata.size as u64;

        let mut found = 0usize;
        for s in &out.sections {
            if s.name == "rodata" {
                continue;
            }
            for i in (0..s.size).step_by(4) {
                let addr = s.base + i as u64;
                let at = (addr - machine_layout::IMAGE_BASE) as usize;
                if at + 4 > out.blob.len() {
                    break;
                }
                let w = u32::from_le_bytes(out.blob[at..at + 4].try_into().expect("4 bytes"));
                if w & 0x9F00_0000 != 0x1000_0000 {
                    continue;
                }
                let imm = (((w >> 5) & 0x7FFFF) << 2) | ((w >> 29) & 0x3);
                let imm = ((imm << 11) as i32) >> 11;
                let target = (addr as i64 + imm as i64) as u64;
                assert!(
                    (rodata.base..rodata_end).contains(&target),
                    "section `{}` +{i}: a resolved ADR points at {target:#x}, outside rodata \
                     [{:#x},{rodata_end:#x})",
                    s.name,
                    rodata.base
                );
                found += 1;
            }
        }
        assert_eq!(
            found, sites,
            "every emitted Reloc::RodataAdr site must appear in the blob as a resolved ADR"
        );
    }

    #[test]
    fn layout_places_entry_then_code_then_abort_when_rodata_is_empty() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0xAABB_CCDD]));
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        let out = layout_program(&program, None).unwrap();
        let names: Vec<&str> = out.sections.iter().map(|s| s.name).collect();
        assert_eq!(names, ["entry", "code", "abort", "checkpoint"]);
        assert_eq!(out.entry, machine_layout::IMAGE_BASE);
        let last = out.sections.last().unwrap();
        assert_eq!(
            out.blob.len() as u64,
            last.base + last.size - machine_layout::IMAGE_BASE
        );
    }

    #[test]
    fn layout_places_rodata_between_code_and_abort_when_nonempty() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0x1111_1111]));
        let program = CodegenProgram {
            fns,
            rodata: vec![b"hello".to_vec()],
            ..Default::default()
        };
        let out = layout_program(&program, None).unwrap();
        let names: Vec<&str> = out.sections.iter().map(|s| s.name).collect();
        assert_eq!(names, ["entry", "code", "rodata", "abort", "checkpoint"]);
        let rodata = out.sections.iter().find(|s| s.name == "rodata").unwrap();
        assert_eq!(rodata.base % 8, 0);
        assert_eq!(rodata.size, 5);
    }

    #[test]
    fn call_reloc_resolves_to_the_callees_own_base() {
        let mut fns = BTreeMap::new();
        let mut g = fn_words(&[encode::enc_bl(0)]);
        g.relocs.push(Reloc::Call {
            word: 0,
            key: "f".to_string(),
        });
        fns.insert("f".to_string(), fn_words(&[0x1111_1111, 0x2222_2222]));
        fns.insert("g".to_string(), g);
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        let out = layout_program(&program, None).unwrap();
        let code = out.sections.iter().find(|s| s.name == "code").unwrap();
        let g_word0_addr = code.base + 8;
        let f_base = code.base;
        let delta = f_base as i64 - g_word0_addr as i64;
        let expect = encode::enc_bl(delta as i32);
        let word_index_in_blob = ((g_word0_addr - machine_layout::IMAGE_BASE) / 4) as usize;
        let bytes = &out.blob[word_index_in_blob * 4..word_index_in_blob * 4 + 4];
        let got = u32::from_le_bytes(bytes.try_into().unwrap());
        assert_eq!(got, expect);
    }

    #[test]
    fn unresolved_call_target_is_an_internal_error() {
        let mut fns = BTreeMap::new();
        let mut g = fn_words(&[0]);
        g.relocs.push(Reloc::Call {
            word: 0,
            key: "missing".to_string(),
        });
        fns.insert("g".to_string(), g);
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        assert!(layout_program(&program, None).is_err());
    }

    use crate::eval::image_checks::PoolBacking;

    fn backing(name: &str, bytes: u64, align: u64, device: Option<usize>) -> PoolBacking {
        PoolBacking {
            name: name.to_string(),
            is_dma: device.is_some(),
            payload: "Hdr".to_string(),
            slots: 1,
            slot_bytes: bytes,
            bytes,
            align,
            device,
        }
    }

    fn backings(list: Vec<PoolBacking>) -> BTreeMap<String, PoolBacking> {
        list.into_iter().map(|b| (b.name.clone(), b)).collect()
    }

    #[test]
    fn pools_are_placed_in_name_order_each_at_its_own_alignment() {
        let m = backings(vec![
            backing("Zeta", 3, 1, None),
            backing("Alpha", 5, 8, Some(0)),
            backing("Mid", 2, 2, None),
        ]);
        let (pools, base, size, end) = place_pools(0x1004, &[], &m)
            .expect("no section overlaps")
            .expect("three pools reserve a section");
        assert_eq!(base, 0x1008, "the section itself is 8-byte aligned");
        let names: Vec<&str> = pools.iter().map(|p| p.backing.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Mid", "Zeta"]);
        assert_eq!(pools[0].base, 0x1008);
        assert_eq!(pools[1].base, 0x100e);
        assert_eq!(pools[2].base, 0x1010);
        assert_eq!(end, 0x1013);
        assert_eq!(size, end - base);
        assert_eq!(
            place_pools(0x1004, &[], &m).unwrap(),
            Some((pools, base, size, end))
        );
    }

    #[test]
    fn pool_backing_placed_inside_an_existing_section_is_refused() {
        let m = backings(vec![backing("A", 16, 8, Some(0))]);
        let sections = vec![Section {
            name: "rtdata",
            base: 0x1000,
            size: 0x100,
        }];
        let err = place_pools(0x1080, &sections, &m).expect_err("0x1080 is inside rtdata");
        assert!(
            err.message.contains("inside a section that ends at 0x1100"),
            "{}",
            err.message
        );
        assert!(place_pools(0x1100, &sections, &m).is_ok());
    }

    #[test]
    fn an_image_with_no_pool_reserves_no_pooldata_section() {
        assert_eq!(place_pools(0x1000, &[], &BTreeMap::new()).unwrap(), None);
    }

    #[test]
    fn a_pool_window_outside_the_pooldata_section_is_refused() {
        let sections = vec![
            Section {
                name: "rtdata",
                base: 0x1000,
                size: 0x100,
            },
            Section {
                name: "pooldata",
                base: 0x1100,
                size: 0x40,
            },
        ];
        let inside = vec![PoolPlacement {
            backing: backing("A", 0x40, 8, Some(0)),
            base: 0x1100,
        }];
        verify_pool_windows(&sections, &inside).expect("wholly inside its own section");

        let before = vec![PoolPlacement {
            backing: backing("A", 0x40, 8, Some(0)),
            base: 0x10ff,
        }];
        let err = verify_pool_windows(&sections, &before).expect_err("reaches into rtdata");
        assert!(err.message.contains("not inside the `pooldata` section"));

        let past = vec![PoolPlacement {
            backing: backing("A", 0x41, 8, Some(0)),
            base: 0x1100,
        }];
        let err = verify_pool_windows(&sections, &past).expect_err("runs past the section");
        assert!(err.message.contains("not inside the `pooldata` section"));
    }

    #[test]
    fn two_overlapping_pool_windows_are_refused() {
        let sections = vec![Section {
            name: "pooldata",
            base: 0x1000,
            size: 0x100,
        }];
        let overlapping = vec![
            PoolPlacement {
                backing: backing("A", 0x20, 8, Some(0)),
                base: 0x1000,
            },
            PoolPlacement {
                backing: backing("B", 0x20, 8, None),
                base: 0x1010,
            },
        ];
        let err = verify_pool_windows(&sections, &overlapping).expect_err("A and B overlap");
        assert!(
            err.message.contains("overlapping windows"),
            "{}",
            err.message
        );
    }

    const LANE1_ROW: u64 = 3 * 8 + (crate::rtconfig::METHOD_CALL_POOL_COUNT as u64) * 8;

    fn window_static(name: &str, addr: u64, size: u64) -> PlacedStatic {
        PlacedStatic {
            name: name.to_string(),
            ty: format!("{name}Ty"),
            addr,
            size,
        }
    }

    const LANE2_BYTES: u64 = 8 + (crate::rtconfig::BLOCK_POOL_COUNT as u64) * 8;

    #[test]
    fn device_window_accepts_the_live_lane_pages() {
        assert_eq!(LANE1_ROW, 1048);
        assert_eq!(LANE2_BYTES, 24584);
        for cores in [1u64, 2, 3, 5] {
            let placed = vec![
                window_static("LANE2", 0x4000_8800, LANE2_BYTES),
                window_static("LANE1", 0x4000_e900, cores * LANE1_ROW),
                window_static("RT", 0x4054_0000, 3072),
            ];
            verify_device_window_statics(&placed)
                .unwrap_or_else(|e| panic!("cores={cores}: {}", e.message));
        }
    }

    #[test]
    fn device_window_refuses_a_stripe_that_reaches_the_next_page() {
        let placed = vec![
            window_static("LANE1", 0x4000_8000, 2 * LANE1_ROW),
            window_static("LANE2", 0x4000_8800, LANE2_BYTES),
        ];
        let err = verify_device_window_statics(&placed).expect_err("LANE1 reaches LANE2");
        assert!(
            err.message.contains("overlap") && err.message.contains("LANE2"),
            "{}",
            err.message
        );
    }

    #[test]
    fn device_window_refuses_a_stripe_past_the_end_of_the_window() {
        let placed = vec![
            window_static("LANE2", 0x4000_8800, LANE2_BYTES),
            window_static("LANE1", 0x4000_e900, 6 * LANE1_ROW),
        ];
        let err = verify_device_window_statics(&placed).expect_err("6 rows leave the window");
        assert!(
            err.message
                .contains("past the end of the reserved device-page window"),
            "{}",
            err.message
        );
    }

    #[test]
    fn placed_windows_with_no_pooldata_section_are_refused() {
        let sections = vec![Section {
            name: "code",
            base: 0x1000,
            size: 0x100,
        }];
        let pools = vec![PoolPlacement {
            backing: backing("A", 0x10, 8, Some(0)),
            base: 0x1000,
        }];
        let err = verify_pool_windows(&sections, &pools).expect_err("no section to be inside of");
        assert!(err.message.contains("reserves no `pooldata` section"));
    }

    #[test]
    fn only_device_reachable_pools_become_blkpool_windows() {
        let layout = ImageLayout {
            blob: Vec::new(),
            linked: None,
            entry: 0x1000,
            sections: vec![Section {
                name: "pooldata",
                base: 0x2000,
                size: 0x30,
            }],
            runtime: None,
            device_regs: Vec::new(),
            irq_host_injects: Vec::new(),
            core_entries: Vec::new(),
            cores: 1,
            placed_statics: Vec::new(),
            pools: vec![
                PoolPlacement {
                    backing: backing("Control", 0x10, 8, Some(0)),
                    base: 0x2000,
                },
                PoolPlacement {
                    backing: backing("Scratch", 0x20, 8, None),
                    base: 0x2010,
                },
            ],
            blk: None,
        };
        let mut out = String::new();
        render_layout_section(&mut out, &layout);
        let blk: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("BlkPool "))
            .collect();
        assert_eq!(
            blk,
            vec!["BlkPool name=Control device=device#0 base=0x2000 size=0x10"]
        );
        assert_eq!(
            out.lines()
                .filter(|l| l.trim_start().starts_with("Pool name="))
                .count(),
            2
        );
        assert!(out.contains("Pool name=Scratch kind=image"));
        assert!(out.contains("device=none"));
    }

    #[test]
    fn verify_section_sizes_accepts_a_contiguous_table() {
        let sections = vec![
            Section {
                name: "entry",
                base: 0x1000,
                size: 16,
            },
            Section {
                name: "code",
                base: 0x1010,
                size: 32,
            },
        ];
        assert!(verify_section_sizes(&sections, 0x1000, 48).is_ok());
    }

    #[test]
    fn same_region_is_the_span_property_not_the_base_property() {
        assert!(same_region_holds(0x4050_0050, 0x4051_5610));
        assert!(!same_region_holds(
            0x4060_0000,
            0x4060_0000 + REGION_BYTES + 4
        ));
        assert!(same_region_holds(0x4050_0050, 0x405F_FFFC));
        assert!(!same_region_holds(0x405F_FFF0, 0x4060_0010));
    }

    #[test]
    fn the_region_constant_agrees_with_the_cost_table() {
        let table = crate::cost::load_default().expect("cost table");
        let row = table
            .branch_row("region_bytes")
            .expect("[branch.region_bytes] is SOG §4.8's region size");
        assert_eq!(row.value, REGION_BYTES);
    }

    #[test]
    fn verify_branch_region_refuses_a_straddling_text_span() {
        let straddle = vec![
            Section {
                name: "entry",
                base: 0x405F_FFF0,
                size: 16,
            },
            Section {
                name: "code",
                base: 0x4060_0000,
                size: 16,
            },
        ];
        let err = verify_branch_region(&straddle).expect_err("straddling must fail closed");
        assert!(err.message.contains("straddles"), "{}", err.message);

        let real = vec![
            Section {
                name: "entry",
                base: 0x4050_0000,
                size: 80,
            },
            Section {
                name: "code",
                base: 0x4050_0050,
                size: 85136,
            },
            Section {
                name: "abort",
                base: 0x4051_4ff4,
                size: 120,
            },
            Section {
                name: "checkpoint",
                base: 0x4051_506c,
                size: 28,
            },
        ];
        assert!(verify_branch_region(&real).is_ok());
    }

    #[test]
    fn verify_section_sizes_rejects_a_wrong_blob_length() {
        let sections = vec![Section {
            name: "entry",
            base: 0x1000,
            size: 16,
        }];
        assert!(verify_section_sizes(&sections, 0x1000, 8).is_err());
    }

    #[test]
    fn verify_section_sizes_accepts_steered_rtdata_gap() {
        let sections = vec![
            Section {
                name: "checkpoint",
                base: machine_layout::IMAGE_BASE,
                size: 0x100,
            },
            Section {
                name: "rtdata",
                base: machine_layout::RTDATA_BASE,
                size: 32,
            },
        ];
        let blob_len = machine_layout::RTDATA_BASE + 32 - machine_layout::IMAGE_BASE;
        assert!(verify_section_sizes(&sections, machine_layout::IMAGE_BASE, blob_len).is_ok());
    }

    #[test]
    fn verify_section_sizes_rejects_unsteered_wide_gap() {
        let sections = vec![
            Section {
                name: "a",
                base: 0x1000,
                size: 16,
            },
            Section {
                name: "b",
                base: 0x1100,
                size: 16,
            },
        ];
        assert!(verify_section_sizes(&sections, 0x1000, 0x1100 + 16 - 0x1000).is_err());
    }

    #[test]
    fn verify_section_sizes_rejects_overlap() {
        let sections = vec![
            Section {
                name: "a",
                base: 0x1000,
                size: 16,
            },
            Section {
                name: "b",
                base: 0x1008,
                size: 16,
            },
        ];
        assert!(verify_section_sizes(&sections, 0x1000, 24).is_err());
    }

    #[test]
    fn verify_section_sizes_rejects_first_section_not_at_image_base() {
        let sections = vec![Section {
            name: "entry",
            base: 0x2000,
            size: 16,
        }];
        assert!(verify_section_sizes(&sections, 0x1000, 16).is_err());
    }

    #[test]
    fn layout_program_is_deterministic() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0x1111_1111]));
        let program = CodegenProgram {
            fns,
            rodata: vec![b"x".to_vec()],
            ..Default::default()
        };
        let a = layout_program(&program, None).unwrap();
        let b = layout_program(&program, None).unwrap();
        assert_eq!(a, b);
    }

    fn program_with_rodata(longest: &[u8]) -> CodegenProgram {
        CodegenProgram {
            fns: BTreeMap::new(),
            rodata: vec![longest.to_vec()],
            ..Default::default()
        }
    }

    #[test]
    fn transcript_bound_counts_one_line_per_test_plus_the_summary() {
        let program = program_with_rodata(b"boom");
        let tests = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let bound = compute_transcript_bound(&program, &tests);
        assert_eq!(bound.lines, 7);
    }

    #[test]
    fn transcript_bound_grows_with_the_longest_rodata_string() {
        let short = program_with_rodata(b"x");
        let long = program_with_rodata(&vec![b'x'; 500]);
        let tests = vec!["t".to_string()];
        let short_bound = compute_transcript_bound(&short, &tests);
        let long_bound = compute_transcript_bound(&long, &tests);
        assert!(long_bound.worst_case_bytes > short_bound.worst_case_bytes);
        assert_eq!(
            long_bound.worst_case_bytes - short_bound.worst_case_bytes,
            2 * (500 - DEADLOCK_MSG.len() as u64)
        );
    }

    #[test]
    fn transcript_bound_with_no_rodata_still_covers_the_deadlock_message() {
        let program = CodegenProgram {
            fns: BTreeMap::new(),
            rodata: Vec::new(),
            ..Default::default()
        };
        let tests = vec!["only_test".to_string()];
        let bound = compute_transcript_bound(&program, &tests);
        let failed_len = 7 + 2 * DEADLOCK_MSG.len() as u64 + 20 + 1;
        const LANE1_SCALAR: u64 = 12 + 20 + 9 + 20 + 10 + 20 + 1;
        const LANE1_QUIESCE: u64 = 21 + 1;
        let lane1_hits = 11 + lane1_pair_bytes() + 1;
        assert_eq!(
            bound.worst_case_bytes,
            16 + failed_len + 57 + LANE1_SCALAR + LANE1_QUIESCE + lane1_hits
        );
    }

    #[test]
    fn lane_hit_reservations_over_approximate_the_widest_printable_line() {
        const COUNT_DIGITS: u64 = 20;

        let n = crate::rtconfig::METHOD_CALL_POOL_COUNT as u64;
        let lane1_widest = n * (3 + 1 + COUNT_DIGITS) + (n - 1);
        assert!(
            lane1_pair_bytes() >= lane1_widest,
            "lane 1 reservation {} must cover the widest printable pair list {lane1_widest}",
            lane1_pair_bytes()
        );
        assert_eq!(
            lane1_pair_bytes() - lane1_widest,
            1,
            "and must not be loose by more than the one byte the trailing-comma \
             over-charge costs"
        );

        let pairs = crate::rtconfig::BLOCK_BOUND_PRINT_PAIRS as u64;
        let lane2_widest = pairs * (4 + 1 + COUNT_DIGITS) + (pairs - 1);
        assert!(
            lane2_pair_bytes() >= lane2_widest,
            "lane 2 reservation {} must cover the widest printable pair list {lane2_widest}",
            lane2_pair_bytes()
        );
        assert_eq!(lane2_pair_bytes() - lane2_widest, 1);

        let marker_widest = " truncated=".len() as u64 + 4;
        assert!(
            lane2_marker_bytes() >= marker_widest,
            "the truncation marker must be reserved for, not discovered at run time"
        );

        let lane1_line = 11 + lane1_pair_bytes() + 1;
        let lane2_line = 11 + lane2_pair_bytes() + lane2_marker_bytes() + 1;
        assert_eq!(lane1_line, 3212, "lane 1 hits line reservation");
        assert_eq!(lane2_line, 3355, "lane 2 hits line reservation");
        assert!(
            lane1_line + lane2_line < console::DATA_SIZE,
            "both hit lines together must leave room for the test/summary lines"
        );
    }

    #[test]
    fn lane2_reservation_is_bounded_by_the_print_pair_cap() {
        let per_pair = lane2_pair_bytes() / (crate::rtconfig::BLOCK_BOUND_PRINT_PAIRS as u64);
        assert_eq!(per_pair, 4 + 1 + 20 + 1, "id digits from BLOCK_POOL_COUNT");
        assert!(
            lane2_pair_bytes() < (crate::rtconfig::BLOCK_POOL_COUNT as u64) * per_pair,
            "the reservation must be the *printable* cap, not the whole pool"
        );
    }

    #[test]
    fn check_transcript_bound_accepts_an_ordinary_small_program() {
        let program = program_with_rodata(b"short message");
        let tests: Vec<String> = (0..10).map(|i| format!("test_{i}")).collect();
        assert!(check_transcript_bound(&program, &tests).is_ok());
    }

    #[test]
    fn check_transcript_bound_rejects_too_many_lines() {
        let program = program_with_rodata(b"x");
        let tests: Vec<String> = (0..(console::QUEUE_SIZE as usize))
            .map(|i| format!("t{i}"))
            .collect();
        let err = check_transcript_bound(&program, &tests).unwrap_err();
        assert!(
            err.message.contains("exceeds the machine's console bound"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn check_transcript_bound_rejects_a_worst_case_byte_overflow() {
        let huge = vec![b'x'; (console::DATA_SIZE) as usize];
        let program = program_with_rodata(&huge);
        let tests = vec!["one_test".to_string()];
        assert!(check_transcript_bound(&program, &tests).is_err());
    }

    #[test]
    fn layout_program_abort_landings_are_halt_only() {
        let mut expected_fixed = Vec::new();
        push_halt(&mut expected_fixed, EXIT_CODE_ABORT_FIXED);
        let mut expected_val = Vec::new();
        push_halt(&mut expected_val, EXIT_CODE_ABORT_VAL);

        let mut fns = BTreeMap::new();
        fns.insert(
            "f".to_string(),
            crate::codegen::CodegenFn {
                frame_size: 0,
                code: vec![crate::cost::EmittedWord::new(
                    encode::enc_ret(30),
                    "ret".to_string(),
                    crate::cost::CostRule::Branch,
                    None,
                    &[30],
                )],
                relocs: Vec::new(),
            },
        );
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        let layout = layout_program(&program, None).unwrap();
        let abort = layout
            .sections
            .iter()
            .find(|s| s.name == "abort")
            .expect("abort section");
        let abort_bytes = (expected_fixed.len() + expected_val.len()) * 4;
        assert_eq!(abort.size, abort_bytes as u64);

        let off = (abort.base - machine_layout::IMAGE_BASE) as usize;
        let mut expected = Vec::new();
        for w in expected_fixed.iter().chain(expected_val.iter()) {
            expected.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(
            &layout.blob[off..off + expected.len()],
            expected.as_slice(),
            "build-image abort landings must be push_halt, not the print path"
        );
    }
}
