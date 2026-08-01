use std::collections::BTreeMap;

use crate::eval::image::ImageGraph;
use crate::mwir::{self, LayoutCtx};
use crate::syntax::ast::Module;

use super::boot_init::{HandleSpace, image_decl_handle_word};
use super::{
    LayoutError, closure_decl_items, closure_imported_types, driver_declares_task,
    driver_task_method_names, irq_bind_handlers_in_driver,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorRuntimeLayout {
    pub name: String,
    pub mailbox_capacity: u64,
    pub slot_size: u64,
    pub state_size: u64,
    pub frame_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverRuntimeLayout {
    pub name: String,
    pub state_size: u64,
    pub has_wake: bool,
    pub wake_drain_index: Option<usize>,
    pub mailbox: Option<DriverMailbox>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverMailbox {
    pub capacity: u64,
    pub slot_size: u64,
    pub frame_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeTables {
    pub actors: Vec<ActorRuntimeLayout>,
    pub drivers: Vec<DriverRuntimeLayout>,
    pub free_turns: Vec<(String, u64)>,
    pub n_turns: u64,
    pub turn_stride: u64,
    pub ready_queue_capacity: u64,
    pub group_arena_capacity: u64,
    pub group_max_children: usize,
    pub rings: Vec<RingLayout>,
    pub ring_stride: u64,
    pub rings_padding: u64,
    pub cores: usize,
    pub total_bytes: u64,
    pub select_by_core: Vec<Vec<String>>,
    pub drain_by_core: Vec<bool>,
    pub child_sites: Vec<(String, usize, usize)>,
    pub ring_target_handles: Vec<u64>,
    pub enqueue_handles: Vec<u64>,
    pub enqueue_actors: Vec<String>,
    pub root_methods: Vec<Vec<(String, bool, bool)>>,
    pub root_cores: Vec<usize>,
    pub n_boot_calls: usize,
    pub irq_vector_bits: Vec<u64>,
    pub wake_pending_addrs: Vec<u64>,
}

impl RuntimeTables {
    pub fn stripe_for_cores(&mut self, cores: usize) {
        debug_assert!(cores >= 1);
        let old = self.cores as u64;
        let new = cores as u64;
        let per_core = self.ready_queue_capacity * 8 + RR_CURSOR_SIZE;
        self.total_bytes = self.total_bytes - old * per_core + new * per_core;
        self.cores = cores;
    }

    pub fn add_cross_core_rings(&mut self, rings: Vec<RingLayout>) {
        self.ring_stride = ring_data_stride_bytes(&rings);
        self.rings_padding = rings_padding_bytes(&rings);
        self.total_bytes += rings_reservation_bytes(&rings);
        self.rings = rings;
    }
}

pub fn ring_data_stride_bytes(rings: &[RingLayout]) -> u64 {
    rings
        .iter()
        .map(|r| r.capacity * r.slot_size)
        .max()
        .unwrap_or(0)
}

pub fn rings_padding_bytes(rings: &[RingLayout]) -> u64 {
    if rings.is_empty() {
        return 0;
    }
    let stride = ring_data_stride_bytes(rings);
    let raw: u64 = rings.iter().map(|r| r.capacity * r.slot_size).sum();
    (rings.len() as u64) * stride - raw
}

pub fn rings_reservation_bytes(rings: &[RingLayout]) -> u64 {
    if rings.is_empty() {
        return 0;
    }
    let n = rings.len() as u64;
    n * MAILBOX_BOOKKEEPING_SIZE + n * ring_data_stride_bytes(rings)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingKind {
    Request,
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingLayout {
    pub src: usize,
    pub dst: usize,
    pub kind: RingKind,
    pub actor: Option<String>,
    pub capacity: u64,
    pub slot_size: u64,
}

impl RingLayout {
    pub fn bytes(&self) -> u64 {
        self.capacity * self.slot_size + MAILBOX_BOOKKEEPING_SIZE
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            RingKind::Request => "request",
            RingKind::Reply => "reply",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingAddrs {
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
}

pub(crate) const REPLY_SLOT_SIZE: u64 = 16;

pub(crate) const MAILBOX_BOOKKEEPING_SIZE: u64 = 3 * 8;
pub(crate) const RR_CURSOR_SIZE: u64 = 8;

pub(crate) fn value_as_u64(v: &crate::eval::value::Value) -> Option<u64> {
    use crate::eval::value::Value;
    match *v {
        Value::U8(n) => Some(n as u64),
        Value::U16(n) => Some(n as u64),
        Value::U32(n) => Some(n as u64),
        Value::U64(n) => Some(n),
        Value::Usize(n) => Some(n as u64),
        Value::I8(n) if n >= 0 => Some(n as u64),
        Value::I16(n) if n >= 0 => Some(n as u64),
        Value::I32(n) if n >= 0 => Some(n as u64),
        Value::I64(n) if n >= 0 => Some(n as u64),
        Value::Isize(n) if n >= 0 => Some(n as u64),
        _ => None,
    }
}

#[derive(Clone)]
pub(crate) struct ActorMethodShape {
    pub(crate) name: String,
    pub(crate) is_async: bool,
    pub(crate) reply_is_aggregate: bool,
    pub(crate) param_sizes: Vec<u64>,
    pub(crate) param_types: Vec<crate::sema::types::Type>,
    pub(crate) ret: crate::sema::types::Type,
    pub(crate) is_task: bool,
    pub(crate) is_handoff: bool,
}

pub(crate) fn merge_actor_pub_methods(
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<BTreeMap<String, Vec<ActorMethodShape>>, LayoutError> {
    use crate::sema::types::{DeclItem, DeclMember};

    let imported = closure_imported_types(modules)
        .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
    let mut out: BTreeMap<String, Vec<ActorMethodShape>> = BTreeMap::new();
    for (addr, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
        let items = crate::sema::types::declare_with_imports(&specialized, &imported[addr])
            .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
        for item in items {
            let DeclItem::Struct(s) = item else { continue };
            if !s.is_actor {
                continue;
            }
            if !s.generics.is_empty() {
                continue;
            }
            let mut methods = Vec::new();
            for m in &s.members {
                let DeclMember::Fn(f) = m else { continue };
                let Some(recv) = &f.receiver else { continue };
                if !recv.is_pub {
                    continue;
                }
                let mut param_sizes = Vec::with_capacity(f.params.len());
                let mut param_types = Vec::with_capacity(f.params.len());
                for p in &f.params {
                    let size = mwir::size_of(&p.ty, layout_ctx).map_err(|e| {
                        LayoutError::new(format!(
                            "actor `{}`'s own `{}` message shape: {e}",
                            s.name, f.name
                        ))
                    })?;
                    param_sizes.push(size as u64);
                    param_types.push(p.ty.clone());
                }
                methods.push(ActorMethodShape {
                    name: f.name.clone(),
                    is_async: f.is_async,
                    reply_is_aggregate: crate::codegen::is_aggregate(&f.ret),
                    param_sizes,
                    param_types,
                    ret: f.ret.clone(),
                    is_task: f.is_task,
                    is_handoff: s.is_driver && crate::sema::handoff::is_handoff_signature(f),
                });
            }
            out.insert(s.name, methods);
        }
    }
    Ok(out)
}

pub fn actor_method_index_tables(
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<BTreeMap<String, BTreeMap<String, usize>>, LayoutError> {
    let shapes = merge_actor_pub_methods(modules, layout_ctx)?;
    Ok(shapes
        .into_iter()
        .map(|(actor, methods)| {
            let table = methods
                .into_iter()
                .enumerate()
                .map(|(i, m)| (m.name, i))
                .collect();
            (actor, table)
        })
        .collect())
}

pub fn count_with_group_sites(modules: &BTreeMap<String, Module>) -> u64 {
    use crate::syntax::ast::{FnItem, InitItem, Item, Member, Stmt};

    fn walk_stmts(stmts: &[Stmt], count: &mut u64) {
        for s in stmts {
            match s {
                Stmt::With(w) => {
                    *count += 1;
                    walk_stmts(&w.body, count);
                }
                Stmt::If(i) => {
                    walk_stmts(&i.then_branch, count);
                    for e in &i.elifs {
                        walk_stmts(&e.body, count);
                    }
                    if let Some(eb) = &i.else_branch {
                        walk_stmts(eb, count);
                    }
                }
                Stmt::Match(m) => {
                    for arm in &m.arms {
                        walk_stmts(&arm.body, count);
                    }
                }
                Stmt::For(f) => walk_stmts(&f.body, count),
                Stmt::While(w) => walk_stmts(&w.body, count),
                Stmt::Defer(d) => {
                    if let crate::syntax::ast::DeferBody::Suite(body) = &d.body {
                        walk_stmts(body, count);
                    }
                }
                Stmt::ComptimeIf(_)
                | Stmt::Assign(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::Return(_, _)
                | Stmt::Pass(_)
                | Stmt::Dmb(_)
                | Stmt::Assert(_)
                | Stmt::Send(_, _)
                | Stmt::Expr(_, _)
                | Stmt::ComptimeAssert(_, _, _) => {}
            }
        }
    }

    fn walk_fn(f: &FnItem, count: &mut u64) {
        if let Some(body) = &f.body {
            walk_stmts(body, count);
        }
    }
    fn walk_init(i: &InitItem, count: &mut u64) {
        walk_stmts(&i.body, count);
    }

    let mut count = 0u64;
    for module in modules.values() {
        for item in &module.items {
            match item {
                Item::Fn(f) => walk_fn(f, &mut count),
                Item::Struct(s) => {
                    for m in &s.members {
                        match m {
                            Member::Fn(f) => walk_fn(f, &mut count),
                            Member::Init(i) => walk_init(i, &mut count),
                            Member::Field(_) | Member::Pool(_) | Member::ComptimeIf(_) => {}
                        }
                    }
                }
                Item::Const(_)
                | Item::Enum(_)
                | Item::Pool(_)
                | Item::ComptimeIf(_)
                | Item::Static(_) => {}
            }
        }
    }
    count
}

pub(crate) fn turn_owner<'k>(key: &'k str, actor_names: &[String]) -> Option<&'k str> {
    key.split_once('.')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| actor_names.iter().any(|a| a == prefix))
}

pub(crate) fn declared_mailbox_capacity(
    args: &[crate::eval::image::DeclArg],
    who: &str,
) -> Result<Option<u64>, String> {
    let Some(arg) = args.iter().find(|a| a.label == "mailbox") else {
        return Ok(None);
    };
    let capacity = value_as_u64(&arg.value).ok_or_else(|| {
        format!("{who}'s own `mailbox=` value is not a plain non-negative integer")
    })?;
    Ok(Some(capacity))
}

pub fn mailbox_root_names(tables: &RuntimeTables) -> Vec<String> {
    let mut out: Vec<String> = tables.actors.iter().map(|a| a.name.clone()).collect();
    for d in &tables.drivers {
        if d.mailbox.is_some() {
            out.push(d.name.clone());
        }
    }
    out
}

pub(crate) fn why_forbidden_across_a_driver_mailbox(found: &str) -> &'static str {
    if found.starts_with("InterruptCell") {
        return ". 03-hardware.md §6: `InterruptCell[T]` is \"the sole ISR/ordinary-code \
                channel\", interrupt-atomic with respect to every vector that may touch the \
                cell — a channel between this driver's ISR and this driver's own ordinary code. \
                A mailbox is a different channel between different principals, and a cell that \
                crosses it is a second, unordered one. Export the value the cell holds, not the \
                cell";
    }
    if found.starts_with("Receipt") {
        return ". 03-hardware.md §5 gives a receipt one owner and one resolution: the caller \
                holds it and `await`s it; the driver's own bottom half resolves it. A mailbox \
                message posting one back into the driver would let an arbitrary sender name a \
                slot in this driver's queue. The handoff direction — a `Receipt[P]` *reply* \
                from a public synchronous method with exactly one `take p: P` parameter — is \
                the convention §5 blesses, and is accepted";
    }
    ", which 03-hardware.md §1 keeps inside the driver (\"a driver may export safe actor APIs \
     but never raw capabilities\")"
}

pub(crate) fn check_driver_message_surface(
    driver: &str,
    methods: &[ActorMethodShape],
    modules: &BTreeMap<String, Module>,
    decl_items: &[crate::sema::types::DeclItem],
) -> Result<(), String> {
    let bare = driver.split('[').next().unwrap_or(driver);
    let tasks = driver_task_method_names(modules, driver);
    let isrs = irq_bind_handlers_in_driver(modules, bare);
    for m in methods {
        for (i, ty) in m.param_types.iter().enumerate() {
            let Some(found) = crate::sema::types::driver_message_forbidden_carried(ty, decl_items)
            else {
                continue;
            };
            return Err(format!(
                "`@driver` `{driver}` is declared with `mailbox=`, so its `pub` method \
                 `{driver}.{}` is a message shape — and parameter #{} carries `{found}`{}",
                m.name,
                i + 1,
                why_forbidden_across_a_driver_mailbox(&found)
            ));
        }
        if !m.is_handoff {
            if let Some(found) =
                crate::sema::types::driver_message_forbidden_carried(&m.ret, decl_items)
            {
                return Err(format!(
                    "`@driver` `{driver}` is declared with `mailbox=`, so its `pub` method \
                     `{driver}.{}` is a message shape — and its reply carries `{found}`{}",
                    m.name,
                    why_forbidden_across_a_driver_mailbox(&found)
                ));
            }
        }
        if m.is_task || tasks.iter().any(|t| *t == m.name) {
            return Err(format!(
                "`@driver` `{driver}` is declared with `mailbox=`, but its `@task` bottom half \
                 `{driver}.{}` is `pub` — 03-hardware.md §6: a bottom half is woken by an ISR \
                 and drains completions, it is not a message. One turn body cannot have both \
                 entry paths; make it private",
                m.name
            ));
        }
        if isrs.iter().any(|h| *h == m.name) {
            return Err(format!(
                "`@driver` `{driver}` is declared with `mailbox=`, but its interrupt handler \
                 `{driver}.{}` is `pub` — 03-hardware.md §6: an ISR runs in the restricted \
                 interrupt effect set against its own device's registers, never as an admitted \
                 turn on behalf of a sender. Make it private",
                m.name
            ));
        }
    }
    Ok(())
}

pub fn compute_runtime_tables(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
    async_frames: &BTreeMap<String, u64>,
    group_max_children: usize,
) -> Result<Option<RuntimeTables>, String> {
    if graph.actors.is_empty() && graph.drivers.is_empty() && async_frames.is_empty() {
        return Ok(None);
    }
    let shapes = merge_actor_pub_methods(modules, layout_ctx).map_err(|e| e.message)?;
    let mut actor_names: Vec<String> = graph
        .actors
        .iter()
        .map(|d| crate::sema::types::render_type(&d.actor_type))
        .collect();
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        if declared_mailbox_capacity(&decl.args, &format!("driver `{name}`"))?.is_some() {
            actor_names.push(name);
        }
    }

    let mut actors = Vec::with_capacity(graph.actors.len());
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
        let state_size = mwir::size_of(&decl.actor_type, layout_ctx)
            .map_err(|e| format!("actor `{name}`'s own state: {e}"))?
            as u64;
        let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
        let max_args_bytes = methods
            .iter()
            .map(|m| m.param_sizes.iter().sum::<u64>())
            .max()
            .unwrap_or(0);
        let slot_size = 16 + max_args_bytes;
        let max_async_frame = async_frames
            .iter()
            .filter(|(key, _)| turn_owner(key, &actor_names) == Some(name.as_str()))
            .map(|(_, &bytes)| bytes)
            .max()
            .unwrap_or(0);
        actors.push(ActorRuntimeLayout {
            name,
            mailbox_capacity,
            slot_size,
            state_size,
            frame_size: crate::codegen::TURN_RECORD_SIZE + max_async_frame,
        });
    }

    let mut decl_items: Option<Vec<crate::sema::types::DeclItem>> = None;
    let mut drivers = Vec::with_capacity(graph.drivers.len());
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let state_size = mwir::size_of(&decl.actor_type, layout_ctx)
            .map_err(|e| format!("driver `{name}`'s own state: {e}"))?
            as u64;
        let has_wake = driver_declares_task(modules, &name);
        let capacity = declared_mailbox_capacity(&decl.args, &format!("driver `{name}`"))?;
        let mailbox = match capacity {
            None => None,
            Some(capacity) => {
                let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
                if decl_items.is_none() {
                    decl_items = Some(closure_decl_items(modules).map_err(|e| e.message)?);
                }
                check_driver_message_surface(
                    &name,
                    methods,
                    modules,
                    decl_items.as_deref().unwrap_or(&[]),
                )?;
                let max_args_bytes = methods
                    .iter()
                    .map(|m| m.param_sizes.iter().sum::<u64>())
                    .max()
                    .unwrap_or(0);
                let max_async_frame = async_frames
                    .iter()
                    .filter(|(key, _)| turn_owner(key, &actor_names) == Some(name.as_str()))
                    .map(|(_, &bytes)| bytes)
                    .max()
                    .unwrap_or(0);
                Some(DriverMailbox {
                    capacity,
                    slot_size: 16 + max_args_bytes,
                    frame_size: crate::codegen::TURN_RECORD_SIZE + max_async_frame,
                })
            }
        };
        drivers.push(DriverRuntimeLayout {
            name,
            state_size,
            has_wake,
            wake_drain_index: None,
            mailbox,
        });
    }

    let free_turns: Vec<(String, u64)> = async_frames
        .iter()
        .filter(|(key, _)| turn_owner(key, &actor_names).is_none())
        .map(|(key, &bytes)| (key.clone(), crate::codegen::TURN_RECORD_SIZE + bytes))
        .collect();

    let messageable_drivers = drivers.iter().filter(|d| d.mailbox.is_some()).count() as u64;
    let ready_queue_capacity = graph.actors.len() as u64 + messageable_drivers + 1;
    let group_arena_capacity = count_with_group_sites(modules);

    let n_turns = actors.len() as u64 + messageable_drivers + free_turns.len() as u64;
    let widest_turn_area = actors
        .iter()
        .map(|a| a.frame_size)
        .chain(
            drivers
                .iter()
                .filter_map(|d| d.mailbox.as_ref())
                .map(|mb| mb.frame_size),
        )
        .chain(free_turns.iter().map(|(_, area)| *area))
        .max()
        .unwrap_or(0);
    let turn_stride = if n_turns == 0 {
        0
    } else {
        1u64 << (64 - (widest_turn_area - 1).leading_zeros())
    };

    let mut total_bytes = 0u64;
    for a in &actors {
        total_bytes += a.state_size + a.mailbox_capacity * a.slot_size + MAILBOX_BOOKKEEPING_SIZE;
    }
    for d in &drivers {
        total_bytes += d.state_size;
        if let Some(mb) = &d.mailbox {
            total_bytes += mb.capacity * mb.slot_size + MAILBOX_BOOKKEEPING_SIZE;
        }
    }
    total_bytes += n_turns * turn_stride;
    let group_max_children = group_max_children.max(crate::codegen::GROUP_MAX_CHILDREN_FLOOR);
    let group_slot = crate::codegen::group_slot_size(group_max_children);
    total_bytes += ready_queue_capacity * 8 + RR_CURSOR_SIZE + group_arena_capacity * group_slot;

    Ok(Some(RuntimeTables {
        actors,
        drivers,
        free_turns,
        n_turns,
        turn_stride,
        ready_queue_capacity,
        group_arena_capacity,
        group_max_children,
        rings: Vec::new(),
        cores: 1,
        total_bytes,
        ..Default::default()
    }))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct TurnId(u32);

impl TurnId {
    pub fn from_index(index: usize) -> TurnId {
        let biased = u32::try_from(index + 1)
            .expect("a turn array with over 4 billion entries cannot fit the machine's memory");
        TurnId(biased)
    }

    pub fn get(self) -> u32 {
        debug_assert!(self.0 != 0, "TurnId(0) is the None niche, not an id");
        self.0
    }

    pub fn index(self) -> usize {
        debug_assert!(self.0 != 0, "TurnId(0) is the None niche, not an id");
        self.0 as usize - 1
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct GroupId(u32);

impl GroupId {
    pub fn from_index(index: usize) -> GroupId {
        let biased = u32::try_from(index + 1)
            .expect("a group arena with over 4 billion slots cannot fit the machine's memory");
        GroupId(biased)
    }

    pub fn get(self) -> u32 {
        debug_assert!(self.0 != 0, "GroupId(0) is the None niche, not an id");
        self.0
    }

    pub fn index(self) -> usize {
        debug_assert!(self.0 != 0, "GroupId(0) is the None niche, not an id");
        self.0 as usize - 1
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ActorAddrs {
    pub state: u64,
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
    pub turn: u64,
}

impl ActorAddrs {
    pub fn mailbox(&self) -> RingAddrs {
        RingAddrs {
            ring: self.ring,
            head: self.head,
            tail: self.tail,
            count: self.count,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimePlacement {
    pub turns_base: u64,
    pub turn_stride: u64,
    pub turn_ids: BTreeMap<String, TurnId>,
    pub actors: Vec<ActorAddrs>,
    pub drivers: Vec<u64>,
    pub driver_mailboxes: BTreeMap<usize, ActorAddrs>,
    pub free_turns: BTreeMap<String, u64>,
    pub rr_cursors: Vec<u64>,
    pub group_arena: u64,
    pub rings: Vec<RingAddrs>,
    pub wake_base: u64,
}

impl RuntimePlacement {
    pub fn turn_addr(&self, id: TurnId) -> u64 {
        self.turns_base + (id.index() as u64) * self.turn_stride
    }

    pub fn turn_id_for(&self, key: &str, tables: &RuntimeTables) -> Option<TurnId> {
        let roots = mailbox_root_names(tables);
        match turn_owner(key, &roots) {
            Some(root) => {
                if let Some(i) = tables.actors.iter().position(|a| a.name == root) {
                    return Some(TurnId::from_index(i));
                }
                let di = tables.drivers.iter().position(|d| d.name == root)?;
                let rank = self.driver_mailboxes.keys().position(|k| *k == di)?;
                Some(TurnId::from_index(tables.actors.len() + rank))
            }
            None => self.turn_ids.get(key).copied(),
        }
    }

    pub fn turn_area_for(&self, key: &str, tables: &RuntimeTables) -> Option<u64> {
        self.turn_id_for(key, tables).map(|id| self.turn_addr(id))
    }
}

pub fn resolve_runtime_test_args(
    program: &crate::sema::typed::TypedProgram,
    runtime_tests: &[String],
    graph: &crate::eval::image::ImageGraph,
) -> Result<BTreeMap<String, Vec<u64>>, String> {
    let mut out = BTreeMap::new();
    for name in runtime_tests {
        let f = &program.fns[name];
        let mut args = Vec::with_capacity(f.params.len());
        for p in &f.params {
            let crate::sema::types::Type::Named(_, targs) = &p.ty else {
                return Err(format!(
                    "internal error: runtime test `{name}`'s own param `{}` is not an \
                     `Actor[T]` handle (sema should have already rejected this)",
                    p.name
                ));
            };
            let Some(crate::sema::types::TypeArg::Type(inner)) = targs.first() else {
                return Err(format!(
                    "internal error: runtime test `{name}`'s own `Actor[T]` param `{}` has no \
                     type argument",
                    p.name
                ));
            };
            let target_name = crate::sema::types::render_type(inner);
            let space = HandleSpace::from_graph(graph);
            let mut candidates: Vec<String> = Vec::new();
            let mut actor_index: Option<usize> = None;
            for (i, a) in graph.actors.iter().enumerate() {
                if crate::sema::types::render_type(&a.actor_type) == target_name {
                    candidates.push(format!("actor#{i}"));
                    actor_index = Some(i);
                }
            }
            let mut driver_index: Option<usize> = None;
            for (i, d) in graph.drivers.iter().enumerate() {
                if crate::sema::types::render_type(&d.actor_type) == target_name {
                    candidates.push(format!("driver#{i}"));
                    if d.args.iter().any(|a| a.label == "mailbox") {
                        driver_index = Some(i);
                    }
                }
            }
            if candidates.len() != 1 {
                return Err(format!(
                    "runtime test `{name}`'s own parameter `{}: Actor[{target_name}]` needs \
                     exactly one declared `{target_name}` instance in this image; found {} ({})",
                    p.name,
                    candidates.len(),
                    if candidates.is_empty() {
                        "none".to_string()
                    } else {
                        candidates.join(", ")
                    }
                ));
            }
            let Some(idx) = actor_index
                .and_then(|i| {
                    image_decl_handle_word(space, &crate::eval::image::ImageDeclRef::Actor(i))
                })
                .or_else(|| {
                    driver_index.and_then(|i| {
                        image_decl_handle_word(space, &crate::eval::image::ImageDeclRef::Driver(i))
                    })
                })
            else {
                return Err(format!(
                    "runtime test `{name}`'s own parameter `{}: Actor[{target_name}]` resolves \
                     to a `@driver` declared with no `mailbox=` — a driver is messageable only \
                     when its declaration says so (05-library.md §9), so there is nothing for \
                     this handle to call. Add `mailbox=n` to `img.driver({target_name}, ...)`",
                    p.name
                ));
            };
            args.push(idx);
        }
        out.insert(name.clone(), args);
    }
    Ok(out)
}
