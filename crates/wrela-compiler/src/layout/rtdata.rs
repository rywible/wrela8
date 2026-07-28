//! Runtime-table sizing and `rtdata` address types (plans/M6.md item C).

use std::collections::BTreeMap;

use crate::eval::image::ImageGraph;
use crate::mwir::{self, LayoutCtx};
use crate::syntax::ast::Module;

use super::boot_init::{HandleSpace, image_decl_handle_word};
use super::{
    LayoutError, closure_decl_items, closure_imported_types, driver_declares_task,
    driver_task_method_names, irq_bind_handlers_in_driver,
};

// ===========================================================================
// plans/M6.md item C, decision 3: static actor runtime-table sizing — a
// pure function of `(graph, modules, layout_ctx)`, computed once here and
// consumed by both `layout_program`/`layout_test_image` (the `rtdata`
// reservation) and `render_layout_section` (the report's own accounting
// facts) via `ImageLayout::runtime`, so the two can never drift apart.

/// One actor declaration's own static sizing (04 §2/§7, decision 3):
/// facts only, never a placeholder — every field here is a real byte
/// count this image's own `rtdata` section actually reserves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorRuntimeLayout {
    /// The declared actor struct's own name (`types::render_type` of
    /// `ActorDecl::actor_type`) — matches the report's own existing
    /// `Actor index=N type=<name>` line so a reader can correlate the two
    /// without a shared index scheme.
    pub name: String,
    /// The declared `mailbox=` bound (02 §9.2: "the compiler derives the
    /// capacity from the closed sender set" — M6 ships the dumbest sound
    /// floor, the *declared* bound, not derivation; recorded honestly in
    /// plans/M6.md, mailbox capacity *derivation* is explicitly OUT of
    /// M6's own scope line).
    pub mailbox_capacity: u64,
    /// One ring slot's own byte size: 16 (the method-index tag plus the
    /// admitted message's own **waker** word — the awaiting turn's turn
    /// area address, or 0 for a one-way `send`; `codegen::OFF_TURN_*`'s
    /// own module doc has the whole park-and-resume contract) plus the
    /// widest of this actor's own `pub` methods' param blobs (each
    /// param's own `mwir::size_of`, summed). A method with zero params
    /// (or an actor with no `pub` methods at all) still costs the
    /// 16-byte tag+waker pair alone — the minimum a slot can ever be.
    pub slot_size: u64,
    /// This actor struct's own field storage (`mwir::size_of` over the
    /// struct's own field list, `LayoutCtx`) — where the actor instance
    /// itself lives (decision 3's own "fixed data-section slot per
    /// declared actor instance" answer, recorded in plans/M6.md).
    pub state_size: u64,
    /// This actor's own **turn area**: the fixed turn record
    /// (`codegen::TURN_RECORD_SIZE` — busy/suspended/resume_ready/reply/
    /// waker/cur_method/reply_slot/reply_tag) plus the widest persistent frame any of its own
    /// `pub async fn` methods needs (`codegen::async_frame_sizes` — 04
    /// §2's "statically reserved frame slots", where a parked turn's
    /// live temps actually survive its `ret` to the scheduler). One area
    /// per actor, never one per queued message: non-reentrancy caps
    /// in-flight activations at one (item C's frame-arena reading,
    /// unchanged — now finally holding real async locals, the growth
    /// item C's own note predicted).
    pub frame_size: u64,
}

/// Every actor's own sizing, plus the scheduler's own fixed-capacity
/// tables (plans/M6.md item C, decision 3's own "Scheduler tables"
/// paragraph). `total_bytes` is the exact `rtdata` section size
/// `layout_program`/`layout_test_image` reserve — computed once, here,
/// from the per-actor/per-table sizes below, so the reservation and the
/// report's own totals line can never disagree.
/// One `@driver` declaration's own static sizing (plans/M7.md item H1).
/// Deliberately *not* an `ActorRuntimeLayout`: a driver is an actor root
/// (02 §9.1) and, since plans/M8.md item D, may own a mailbox — but only
/// when its declaration says `mailbox=` (05-library.md §9). The state
/// bytes are unconditional; the mailbox half is `Option`, so a driver
/// with no `mailbox=` still has exactly one static fact and no zeroed
/// ring/slot/frame numbers pretending to be decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverRuntimeLayout {
    pub name: String,
    /// This driver struct's own field storage (`mwir::size_of`) — where
    /// the instance lives, exactly like an actor's `state_size`.
    pub state_size: u64,
    /// True when the driver declares a `@task`. Sticky wake-pending bits
    /// live in the contiguous `WAKE.wake_pending` array (M12 item D /
    /// decisions 880–882), not as a trailing word of driver state.
    pub has_wake: bool,
    /// Index into `wake_pending_addrs` / `WAKE.wake_pending` for this
    /// driver's first `@task` drain. Filled by `fill_checkpoint_irq_facts`.
    pub wake_drain_index: Option<usize>,
    /// plans/M8.md item D (decision 19): present exactly when this
    /// declaration carried `mailbox=n`. The three numbers are the same
    /// three an `ActorRuntimeLayout` carries and are computed by the same
    /// arithmetic — a messageable driver is admitted, selected and
    /// dispatched by the identical routines an `@actor` is
    /// (`build_rt_enqueue` / `build_rt_select_and_run_symbolic`), never a
    /// second path.
    pub mailbox: Option<DriverMailbox>,
}

/// The mailbox half of a messageable `@driver` (plans/M8.md item D).
/// Field-for-field the mailbox half of `ActorRuntimeLayout`; kept as its
/// own struct so "has a mailbox" is one `Option`, not three sentinel
/// zeros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverMailbox {
    pub capacity: u64,
    pub slot_size: u64,
    pub frame_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeTables {
    pub actors: Vec<ActorRuntimeLayout>,
    /// plans/M7.md item H1: one entry per declared `@driver` instance, in
    /// `ImageGraph::drivers` order. Until this item a `@driver`'s state
    /// was never reserved and its `init` never called, so no capability of
    /// any kind had bytes at runtime — decision 10's second prerequisite.
    pub drivers: Vec<DriverRuntimeLayout>,
    /// One turn area per **free** async fn (every `FlowWirProgram::fns`
    /// key that is not a declared actor's own method — `@test(runtime)`
    /// roots foremost; a compiled-but-unreachable free `async fn` still
    /// gets one so its own `Reloc::TurnFrameAddr` resolves): `(fn key,
    /// area bytes)` where area = `codegen::TURN_RECORD_SIZE` + that fn's
    /// own persistent frame. The root test turn parks/resumes through
    /// its own area via the identical machinery an actor turn uses —
    /// decision 3's "+1 root" ready-queue slot, now made concrete.
    pub free_turns: Vec<(String, u64)>,
    /// plans/M10.md item 0a (decision 552): how many turn areas this image
    /// has — every actor, every **messageable** driver, every free async
    /// fn. The three sets above, counted once, so `total_bytes` and
    /// `place_runtime_tables` cannot disagree about how many strides they
    /// are accounting for.
    pub n_turns: u64,
    /// plans/M10.md item 0a (decision 552): the **uniform** byte stride
    /// every turn area is reserved at — `round_up_pow2` of the widest raw
    /// area over all three owner kinds, or `0` when `n_turns == 0`.
    ///
    /// The per-owner `frame_size` fields (`ActorRuntimeLayout::frame_size`,
    /// `DriverMailbox::frame_size`, `free_turns.1`) stay the **raw** area —
    /// `TURN_RECORD_SIZE + widest owned async frame` — deliberately. Two
    /// things depend on that: the report's own `frame=` lines say how much
    /// of the stride is real (a reader can subtract and see the padding),
    /// and `build_rt_select_and_run_core`'s lineage-zeroing guard is keyed
    /// on "does this owner actually have frame slots past its record",
    /// which the stride cannot answer — under a uniform stride it would
    /// answer "yes" for a bare actor and emit two stores into padding.
    /// Only the *reservation* (this field) is uniform.
    pub turn_stride: u64,
    /// Ready-queue capacity: every actor plus the one root test turn
    /// (decision 3's own "fixed capacity = actor count + 1 root").
    /// Reserved as a real `u64`-per-slot table; `rt_select_and_run` (below)
    /// does not yet populate it (an O(actor-count) round-robin scan is the
    /// dumbest correct selection at M6's own actor counts, module doc on
    /// `build_rt_select_and_run` below) — reserved now so a later
    /// milestone's real event-driven wake can start using it without a
    /// layout change, exactly like `wrela_machine::machine_info::
    /// OFF_NEXT_DEADLINE`'s own precedent.
    pub ready_queue_capacity: u64,
    /// A real (not hand-waved) static count of `with group(...)` sites
    /// found across every raw module in the build closure (decision 3's
    /// own "fixed small global group arena sized from static with-site
    /// count" — `count_with_group_sites`, below, an honest AST-level walk
    /// that does not descend into closure bodies or `comptime if`
    /// branches; recorded as a disclosed gap in plans/M6.md, inert at C
    /// since no group actually executes against this arena until item F
    /// wires it). Always `0` in today's report-bearing corpus (no
    /// existing actor-bearing golden uses `with group` yet).
    pub group_arena_capacity: u64,
    /// plans/M12.md item F (decisions 886–889): image
    /// `GROUP_MAX_CHILDREN` fact — `max(FLOOR=2, max g.start children
    /// over group sites)`. Drives `GROUP_SLOT_SIZE = 64 + N*16` and the
    /// generated `GroupSlot` child fields. Floor 2 for empty-arena images.
    pub group_max_children: usize,
    /// plans/M8.md item C2: this image's own cross-core SPSC rings, in
    /// `cross_core_rings`'s canonical order. Empty for every single-core
    /// image and for a cross-core image whose graph has no cross-core
    /// message edge (decision 28's own "emit nothing" rule).
    pub rings: Vec<RingLayout>,
    /// plans/M12.md item C (decision 875): image-wide uniform ring-data
    /// stride in bytes (`max(capacity * slot_size)`), or `0` when there
    /// are no rings. Padding cost is `rings_padding`.
    pub ring_stride: u64,
    /// plans/M12.md item C: `n_rings * ring_stride - sum(capacity *
    /// slot_size)` — bytes spent to buy a uniformly-strided type. Printed
    /// on the report's `Rings` line; folded into `total_bytes` before
    /// `steer_rtdata_base` so `RTDATA_SIZE_MAX` fails closed with the
    /// number.
    pub rings_padding: u64,
    /// How many cores this image brings up (`placement::PlacementTable::
    /// cores` — `1` for every single-core image, `VCPUS` for a cross-core
    /// graph). plans/M8.md item C1: the scheduler's own per-core state is
    /// **striped by this count** — one ready-queue table and one
    /// round-robin cursor per live core, never one global set shared
    /// across cores (04 §2: one event loop *per core*, no migration). A
    /// single-core image stripes by 1, which is byte-for-byte the
    /// pre-C1 reservation.
    pub cores: usize,
    /// The exact `rtdata` section size: every actor's own state + ring +
    /// bookkeeping + frame bytes, plus the per-core ready-queue tables,
    /// the per-core round-robin cursor words, and the group arena.
    pub total_bytes: u64,
    /// M11 F (decision 790): mailbox-root names per live core, in
    /// `mailbox_root_names` order filtered by placement — the RR select
    /// list. Empty until `RuntimeWiring::derive` fills them.
    pub select_by_core: Vec<Vec<String>>,
    /// M11 F: `true` when this core has any inbound cross-core ring.
    pub drain_by_core: Vec<bool>,
    /// M11 F: `(callee_key, child_index, turn_index)` per `g.start` site,
    /// in `BTreeMap` key order.
    pub child_sites: Vec<(String, usize, usize)>,
    /// M11 G (decision 801): image handle word per request ring (parallel
    /// to `rings`); 0 / unused for reply rings.
    pub ring_target_handles: Vec<u64>,
    /// M11 G: mailbox-root handle words in root order (enqueue stubs).
    pub enqueue_handles: Vec<u64>,
    /// M11 G: mailbox-root names parallel to `enqueue_handles`.
    pub enqueue_actors: Vec<String>,
    /// M11 J: per root (enqueue_actors order): `(method_key, is_async, reply_is_aggregate)`.
    pub root_methods: Vec<Vec<(String, bool, bool)>>,
    /// M11 J: placement core per root (enqueue_actors order).
    pub root_cores: Vec<usize>,
    /// M11 H: count of boot `init` calls (drivers then actors with `init`).
    pub n_boot_calls: usize,
    /// M11 I: pending-vector bit indices for sealed IRQ binds (decision 823).
    pub irq_vector_bits: Vec<u64>,
    /// M11 I: absolute wake-pending word addresses (decision 823).
    pub wake_pending_addrs: Vec<u64>,
}

impl RuntimeTables {
    /// Restripes the scheduler tables for `cores` live cores, recomputing
    /// `total_bytes`. Called once, by `RuntimeWiring::derive`, as soon as
    /// placement is known — `compute_runtime_tables` itself cannot call
    /// `placement::place` (placement calls *it* for the per-actor sizes it
    /// packs on, so the dependency runs one way only).
    pub fn stripe_for_cores(&mut self, cores: usize) {
        debug_assert!(cores >= 1);
        let old = self.cores as u64;
        let new = cores as u64;
        let per_core = self.ready_queue_capacity * 8 + RR_CURSOR_SIZE;
        self.total_bytes = self.total_bytes - old * per_core + new * per_core;
        self.cores = cores;
    }

    /// plans/M8.md item C2 / M12 item C: installs this image's own
    /// cross-core rings and grows `total_bytes` by their **padded**
    /// reservation (all CTLs, then uniformly-strided DATA). Called once,
    /// by `RuntimeWiring::derive`, right after `stripe_for_cores` — the
    /// rings are placed **last** in `rtdata` (after the group arena), so
    /// nothing an existing golden pins moves for an image that has none.
    pub fn add_cross_core_rings(&mut self, rings: Vec<RingLayout>) {
        self.ring_stride = ring_data_stride_bytes(&rings);
        self.rings_padding = rings_padding_bytes(&rings);
        self.total_bytes += rings_reservation_bytes(&rings);
        self.rings = rings;
    }
}

/// Image-wide max of `capacity * slot_size` (decision 875). `0` when empty.
pub fn ring_data_stride_bytes(rings: &[RingLayout]) -> u64 {
    rings
        .iter()
        .map(|r| r.capacity * r.slot_size)
        .max()
        .unwrap_or(0)
}

/// `n * stride - sum(raw data)` — the bytes uniform stride spends.
pub fn rings_padding_bytes(rings: &[RingLayout]) -> u64 {
    if rings.is_empty() {
        return 0;
    }
    let stride = ring_data_stride_bytes(rings);
    let raw: u64 = rings.iter().map(|r| r.capacity * r.slot_size).sum();
    (rings.len() as u64) * stride - raw
}

/// CTL block (`n * 24`) plus padded DATA (`n * stride`).
pub fn rings_reservation_bytes(rings: &[RingLayout]) -> u64 {
    if rings.is_empty() {
        return 0;
    }
    let n = rings.len() as u64;
    n * MAILBOX_BOOKKEEPING_SIZE + n * ring_data_stride_bytes(rings)
}

/// plans/M8.md item C2 (04-compiler.md §3: "cross-core actor edges ...
/// lowered to compiler-generated bounded SPSC rings in guest memory").
/// Which of the two lanes a ring is — decision 29 keeps them separate so a
/// request lane's back-pressure can never sit in front of a reply that has
/// nowhere else to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingKind {
    /// `src` -> `dst`: an admitted message, waiting to be handed to
    /// `dst`'s own `__rt_enqueue_<actor>` by `dst`'s drain.
    Request,
    /// `src` -> `dst`: a completed turn's reply word plus the address of
    /// the turn record on `dst` it belongs to.
    Reply,
}

/// One cross-core SPSC ring's own static shape. The producer is core
/// `src` and the consumer is core `dst` — **one producer because one
/// core**, not because one actor (decision 28): two actors on core 0 that
/// both message the same actor on core 1 share one ring, and the baton
/// (decision 11) plus the per-core cooperative loop mean neither can be
/// mid-enqueue when the other starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingLayout {
    pub src: usize,
    pub dst: usize,
    pub kind: RingKind,
    /// The target actor struct, for a `Request` ring: the ring feeds
    /// exactly one mailbox, which is what makes its slot format identical
    /// to that mailbox's own and its capacity derivable from the sealed
    /// graph. `None` for a `Reply` ring (a reply is addressed to a turn
    /// record, not to an actor).
    pub actor: Option<String>,
    pub capacity: u64,
    pub slot_size: u64,
}

impl RingLayout {
    /// Logical ring size: `capacity * slot_size` plus the same three-word
    /// head/tail/count bookkeeping a mailbox carries. The **placed**
    /// reservation after M12 item C is larger when strides differ — see
    /// `rings_reservation_bytes` / `rings_padding_bytes`. Report `bytes=`
    /// still spells this logical size (VMM forge check).
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

/// A bounded ring's four placed addresses — the only thing
/// `build_ring_enqueue` and the drain routines address. A mailbox is one
/// of these (`ActorAddrs::mailbox`) and so is every cross-core ring, which
/// is exactly why the cross-core producer *is* `build_rt_enqueue`'s own
/// body rather than a second implementation of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingAddrs {
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
}

/// A reply slot: the destination turn's own `TurnId` in the low half of
/// word 0, the reply `tag` in the high half (plans/M10.md item J,
/// decision 665 — the upper half was unused padding after 0c1), then the
/// reply word at +8. `REPLY_SLOT_SIZE` stays 16: the tag rides the spare
/// half rather than growing the ring.
pub(crate) const REPLY_SLOT_SIZE: u64 = 16;

// M10 F dissolved `BRK_XREPLY_UNKNOWN_CORE` (0xACD8): specialized
// `emit_rt_select_and_run` dense-matches `xreply_remotes` (decision 557
// typed core tag + decision 630). F2's `build_rt_xreply` still owns the
// ring-full trap above.

/// Per-actor ring bookkeeping beyond the ring bytes themselves: head,
/// tail, count (3 `u64`s). Reply plumbing no longer lives here at all —
/// M6-C's own `last_result` slot (the nested-drain placeholder's side
/// channel) is deleted with the placeholder itself: a completing turn's
/// reply is delivered to the *awaiting turn's* own reply slot
/// (`codegen::OFF_TURN_REPLY`, via the waker carried in the message).
pub(crate) const MAILBOX_BOOKKEEPING_SIZE: u64 = 3 * 8; // head, tail, count
/// The deterministic round-robin cursor word `rt_run_one` reads/advances
/// (04 §2's own tie-breaker) — one `u64`, placed right after the
/// ready-queue table (`RuntimeTables::total_bytes`'s own byte order).
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

/// One actor struct's own shape, re-derived from raw source the same way
/// `merge_layout_ctx`/`mwir::build_layout_ctx` already re-derive
/// `sema::specialize::specialize` -> `sema::types::declare` per module
/// (this module's own established precedent for "recompute from the raw
/// AST rather than thread extra state out of an earlier pass") — the one
/// additional fact `LayoutCtx` does not carry: each `pub` method's own
/// name, `is_async` color, and param types (02 §9.2: only a `pub` method
/// is ever reachable through `Actor[T]`, so only these are message
/// shapes). Keyed by struct name, last-module-wins on a same-spelling
/// collision — the identical disclosed simplification `merge_layout_ctx`
/// already carries.
#[derive(Clone)]
pub(crate) struct ActorMethodShape {
    /// plans/M6.md item D: this method's own bare name — added to this
    /// struct (previously anonymous within its own `Vec`) so
    /// `actor_method_index_tables`, below, can hand out a stable
    /// `(actor, method name) -> dispatch index` table, the exact same
    /// declaration order `dispatch: &[usize]` (`build_rt_select_and_run`)
    /// already numbers methods in.
    pub(crate) name: String,
    /// The method's own color — the dispatch arms in
    /// `rt_select_and_run` read the two return ABIs differently (a sync
    /// method returns its reply in `x0`; an async state machine returns
    /// status in `x0` and, when completed, its reply in `x1` —
    /// `codegen::TURN_STATUS_*`), so every dispatch entry carries this
    /// flag alongside its call target.
    pub(crate) is_async: bool,
    /// plans/M7.md item Z1 (decision 9a): whether this method's own
    /// *declared* reply is an aggregate (`codegen::is_aggregate`, the one
    /// shared ABI predicate — never a copy of it, exactly as
    /// `sema::types::validate_message_shape` already calls it). Such a
    /// method is handed its caller's staging-slot address in `x8` by the
    /// dispatch arm below and writes its reply straight into the awaiting
    /// frame; a scalar-reply method's arm is untouched, down to the word.
    pub(crate) reply_is_aggregate: bool,
    pub(crate) param_sizes: Vec<u64>,
    /// plans/M8.md item D: the message shape itself — every parameter's
    /// declared type plus the declared reply — so the messageable-driver
    /// check (`check_driver_message_surface`) can ask
    /// `sema::types::driver_message_forbidden_carried` about the exact
    /// types the author wrote. Sizes cannot answer an authority question:
    /// a `Receipt[P]`, an `InterruptCell[u32]` and a `u64` are all one
    /// word.
    pub(crate) param_types: Vec<crate::sema::types::Type>,
    pub(crate) ret: crate::sema::types::Type,
    /// 03-hardware.md §6's bottom half. A `@task` is woken by an ISR, not
    /// admitted from a mailbox; a `pub` `@task` on a messageable driver
    /// would give one turn body two entry paths (decision 21).
    pub(crate) is_task: bool,
    /// 03-hardware.md §5's handoff calling convention, recognized by the
    /// one predicate `sema::handoff::is_handoff_signature` (never a copy
    /// of it): a public *synchronous* `@driver` method with exactly one
    /// `take p: P` parameter and result `Receipt[P]`. plans/M8.md item E,
    /// decision 33: it is the one shape whose reply may carry a
    /// `Receipt[P]` across a driver mailbox, because §5 blesses precisely
    /// this signature and nothing else can produce the pair the caller
    /// then `await`s.
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
            // plans/M9.md item MM: only `@actor`/`@driver` structs own
            // message shapes. A generic stdlib type (`List[T, N]`,
            // `SlotMap[T, N]`) is not an actor — sizing its template
            // methods hits bare type parameters and must not run here.
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

/// plans/M6.md item D: `(actor name) -> (method name) -> its own 0-based
/// dispatch index`, in the exact declaration order `merge_actor_pub_methods`
/// (immediately above) already establishes — the same order
/// `build_rt_select_and_run`'s own `dispatch` table numbers methods in, so
/// codegen's own symbolic `Send`/`Await{ActorCall}` lookups
/// (`codegen::ActorMethodIndex`) can never disagree with the runtime
/// dispatch table actually built alongside it.
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

/// A real (not hand-waved) static count of `with group(...)` sites across
/// every raw module in the build closure — `RuntimeTables::
/// group_arena_capacity`'s own doc comment explains the scope and the
/// disclosed gap (closures, `comptime if` branches). Every surviving
/// `Stmt::With` at M6 names a `group(...)` (the scoped-`pool` `with` form
/// stays fail-closed at sema, 02 §10 — plans/M6.md item A's own note), so
/// counting every `Stmt::With` is exact for anything that type-checks.
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

/// Which turn area an async fn's own `Reloc::TurnFrameAddr` (and its
/// sizing) belongs to: a `Struct.method` key whose struct is a declared
/// actor shares that actor's one turn area (non-reentrancy: one turn per
/// actor, whichever method it runs); every other key — free fns
/// (`@test(runtime)` roots foremost), plus any non-actor-owned method
/// key — gets its own dedicated free-turn area. One shared rule so
/// `compute_runtime_tables`'s sizing and `layout`'s reloc resolution can
/// never classify a key differently.
pub(crate) fn turn_owner<'k>(key: &'k str, actor_names: &[String]) -> Option<&'k str> {
    key.split_once('.')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| actor_names.iter().any(|a| a == prefix))
}

/// plans/M8.md item D: the `mailbox=` capacity one declaration carries, or
/// `None` when it carries none. One reader for both `img.actor` (where the
/// label is required — M6 decision 3) and `img.driver` (where it is what
/// makes the driver messageable at all — 05-library.md §9), so the two can
/// never read the same label differently.
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

/// Every root that owns a mailbox, in the one order every consumer walks:
/// each declared `@actor`, then each messageable `@driver`
/// (`graph.drivers` order). `turn_owner`, `place_runtime_tables`,
/// `build_runtime_glue_block` and `RuntimePlacement::turn_area_for` all
/// read this order; a second spelling of it anywhere would be a silent
/// index skew between a turn area and the routine that writes it.
pub fn mailbox_root_names(tables: &RuntimeTables) -> Vec<String> {
    let mut out: Vec<String> = tables.actors.iter().map(|a| a.name.clone()).collect();
    for d in &tables.drivers {
        if d.mailbox.is_some() {
            out.push(d.name.clone());
        }
    }
    out
}

/// plans/M8.md item D, the security surface (decisions 20/21). A
/// messageable `@driver`'s mailbox admits its `pub` methods, so those
/// signatures are message shapes (02-language.md §9.4) and 03-hardware.md
/// §1's "a driver may export safe actor APIs but **never raw
/// capabilities**" is what decides which of them may exist at all.
///
/// Three refusals, each a separate sentence because each names a different
/// leak:
///
/// 1. **Nothing sealed crosses the mailbox, in either direction.** No 03
///    §1 capability, §9 protocol state or §4 sealed queue value may appear
///    in a parameter or reply — and *that* rule is not re-implemented
///    here: `sema::types::validate_fn_capability_types` already refuses
///    every one of them for every `pub` method of an actor or driver,
///    whether or not a mailbox exists (M7 decision 3: "checked where
///    `Actor[T]` already is ... do not build a second mechanism"), and
///    `err-driver-message-capability` pins that it still fires on a
///    *messageable* driver. What this pass adds is the two names that pass
///    lets through, each for its own good reason:
///
///    - **`Receipt[P]`**, which 03 §5 blesses by name for the handoff
///      convention — and a handoff needs the caller-side `await receipt`
///      that plans/M8.md item E makes executable. Refused here, by name,
///      pointing at item E, rather than admitted into a mailbox whose
///      caller cannot resolve what comes back.
///    - **`InterruptCell[T]`** (decision 23), which is not sealed
///      authority at all (M7 decision 17: source-constructible, an
///      `@actor` may hold one) and so is invisible to the containment
///      rules — but which 03 §6 calls "the **sole** ISR/ordinary-code
///      channel". A cell in a message is a second channel between
///      different principals, carrying the interrupt-status word's value
///      to a sender that owns none of §6's ordering.
///
///    Both directions are checked, because for `InterruptCell` both are
///    reachable: the parameter arm is live precisely where sema's is not.
/// 2. **The wrong effect set.** A `@task` bottom half (03 §6) is woken by
///    an ISR and drains completions; it is not a message. Declaring one
///    `pub` on a messageable driver would give one turn body two entry
///    paths — a wake and an admission — and the mailbox path carries none
///    of §6's ordering.
/// 3. **The ISR itself.** An interrupt handler bound with `irq.bind` runs
///    in 03 §6's restricted effect set against its device's registers.
///    Admitting one from a mailbox would run device acknowledge work as an
///    ordinary turn, at an arbitrary time, on behalf of an arbitrary
///    sender.
/// The reason clause both directions of `check_driver_message_surface`
/// append, keyed on which name was found. One writer, so a parameter and a
/// reply carrying the same type can never be told two different stories.
pub(crate) fn why_forbidden_across_a_driver_mailbox(found: &str) -> &'static str {
    if found.starts_with("InterruptCell") {
        // 03-hardware.md §6, quoted rather than paraphrased on "sole",
        // because that word is the whole argument.
        return ". 03-hardware.md §6: `InterruptCell[T]` is \"the sole ISR/ordinary-code \
                channel\", interrupt-atomic with respect to every vector that may touch the \
                cell — a channel between this driver's ISR and this driver's own ordinary code. \
                A mailbox is a different channel between different principals, and a cell that \
                crosses it is a second, unordered one. Export the value the cell holds, not the \
                cell";
    }
    if found.starts_with("Receipt") {
        // plans/M8.md item E, decision 33: item D's floor here refused a
        // receipt in *either* direction, naming this item. The reply
        // direction is now 03-hardware.md §5's blessed handoff and is
        // gone; this is the half that survives, and its reason is
        // narrower than "authority". A receipt names a slot in *this
        // driver's* queue, and 03 §5 gives it exactly one consumer on
        // the driver side — the bottom half's `claim`/`recover`. A sender
        // that could post one back into the mailbox would name a queue
        // slot it does not own, at a time the driver did not choose.
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
        // plans/M8.md item E, decision 33: 03-hardware.md §5's handoff
        // reply is not probed at all, and the reason is structural rather
        // than an exemption. §5's signature is "exactly one `take p: P`
        // parameter and result `Receipt[P]`" — the *same* `P` — so the
        // loop above has already probed the whole payload as parameter
        // #k, with the same walk and the same leaf set. Probing
        // `Receipt[P]` here again could only ever refuse the `Receipt`
        // wrapper, which is the convention itself. Item D's floor
        // (`golden/err-driver-message-receipt`) was this arm refusing that
        // wrapper; it is retargeted at the direction that survives — a
        // receipt as a message *parameter* (05-library.md §2), which the
        // loop above still refuses by name.
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

/// The whole static-sizing pass (module doc above). `Ok(None)` when the
/// image declares no actors AND no async fn exists — the no-placeholder
/// rule: a fully sync image gets no `rtdata` section and no report
/// accounting at all, never a zeroed `RuntimeTables` rendered as if it
/// meant something. `async_frames` is `codegen::async_frame_sizes`'s own
/// result (fn key -> persistent frame bytes), the park-and-resume
/// redesign's one new input: each actor's turn area is sized as the
/// 48-byte record plus the widest of its own async methods' frames, and
/// every non-actor-owned async fn gets its own free-turn area
/// (`RuntimeTables::free_turns`).
pub fn compute_runtime_tables(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
    async_frames: &BTreeMap<String, u64>,
    // plans/M12.md item F: image `GROUP_MAX_CHILDREN` fact (floor 2).
    group_max_children: usize,
) -> Result<Option<RuntimeTables>, String> {
    if graph.actors.is_empty() && graph.drivers.is_empty() && async_frames.is_empty() {
        return Ok(None);
    }
    let shapes = merge_actor_pub_methods(modules, layout_ctx).map_err(|e| e.message)?;
    // plans/M8.md item D: the turn-area owner set is every *mailbox* root,
    // not every actor — a messageable driver's own `pub async fn` parks in
    // its own turn area exactly as an actor's does, and must not be sized
    // as a free turn.
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
        let slot_size = 16 + max_args_bytes; // method idx + waker + args
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

    // plans/M7.md item H1: every declared `@driver` instance's own state
    // bytes, sized by the identical `mwir::size_of` an actor's are — which
    // is only answerable at all since this item taught it that a
    // capability is one word.
    // plans/M7.md item G / M12 item D: a `@task` marks `has_wake`; the
    // sticky wake-pending bit lives in contiguous `WAKE.wake_pending`
    // (decisions 880–882), not a trailing word of driver state.
    // plans/M8.md item D: a `mailbox=` on the declaration makes the driver
    // messageable, and its mailbox is sized by the *identical* arithmetic
    // an actor's is, from the identical `merge_actor_pub_methods` shapes —
    // one mailbox story, not a driver-shaped copy of it.
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
                    // The component table `sealed_authority_carried` walks
                    // spans modules: a plain wrapper struct declared in one
                    // module can carry a capability into a driver method
                    // declared in another.
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

    // "actor count + 1 root" (decision 3), where "actor" is every mailbox
    // root: a messageable driver is selected by the same round-robin tick.
    let messageable_drivers = drivers.iter().filter(|d| d.mailbox.is_some()).count() as u64;
    let ready_queue_capacity = graph.actors.len() as u64 + messageable_drivers + 1;
    let group_arena_capacity = count_with_group_sites(modules);

    // plans/M10.md item 0a (decision 552): every turn area is *reserved* at
    // one image-wide power-of-two stride, so a turn reference can become an
    // index scaled by a shift instead of a bumped address. The stride is
    // `round_up_pow2` of the widest raw area over **all three** owner kinds
    // — actors, messageable drivers, free async fns. Missing one of them
    // undersizes the stride and the array overlaps itself, which is a
    // corrupted transcript rather than a compile error.
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
    // `widest_turn_area >= TURN_RECORD_SIZE` (64) whenever `n_turns > 0`, so
    // the degenerate `n <= 1` cases of the rounding never arise here.
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
    // Every turn area, at the uniform stride — one term, in place of the
    // three per-owner sums it replaces. `place_runtime_tables` bumps the
    // identical stride at the identical three sites; `verify_section_sizes`'
    // blob-length check is what catches the two ever disagreeing.
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
        // Single-core until placement says otherwise (`stripe_for_cores`),
        // and ringless until `add_cross_core_rings` says otherwise.
        // select/drain/child facts filled later by `fill_rtconfig_facts`.
        rings: Vec::new(),
        cores: 1,
        total_bytes,
        ..Default::default()
    }))
}

// ===========================================================================
// Emitted runtime routines (plans/M6.md item C, redesigned by the
// park-and-resume mandate): `rt_enqueue` (admission), `rt_select_and_run`
// (per-actor readiness/selection/dispatch/reply-delivery), `rt_run_one`
// (the round-robin scheduler tick over every actor), and the abandon
// path. Hand-assembled via `Asm` (defined below, M5-E's own tool), the
// identical M5-E style: no asm strings, one instruction encoder call at a
// time, conformance established by real execution (JIT'd against
// host-mmap'd stand-in memory, `harness_jit` below, plus real HVF boots
// in `wrela-vmm`'s own conformance tests) rather than by hand-verifying
// encoded bytes.
//
// ## Why one hand-assembled pair *per actor*, not one generic pair indexed
// by a runtime `actor_idx`
//
// Every actor's own ring/state/bookkeeping address is already a build-time
// constant (`RuntimeTables`/`place_runtime_tables`, above/below) — there is
// no `actor_idx` a real caller would ever need to pass at runtime, so a
// generic address-indexed pair would only add a layer of register-offset
// indirection this milestone's own actor counts never need. 04-compiler.md
// §6's own "Actor as-if" license says exactly this is allowed: "the
// compiler may use direct placement, specialized dispatch tables ...
// provided admission order, non-reentrancy, ... are all preserved" — one
// specialized `rt_enqueue_actor`/`rt_select_actor` pair per actor is that
// license exercised at its simplest.
//
// ## Slot layout (decision 3's "method index + args blob", grown a waker)
//
// `[0..8)`: the admitted message's own method index, a plain `u64`.
// `[8..16)`: the message's own waker — the awaiting turn's turn-area
// address, or 0 for a one-way `send` (`codegen::OFF_TURN_*`'s module doc
// carries the whole contract).
// `[16..slot_size)`: the method's own argument blob, raw 8-byte-per-
// scalar-param words in declared parameter order
// (`ActorRuntimeLayout::slot_size`'s own doc comment) — every param here
// is assumed to fit one 8-byte slot (a disclosed, real simplification: a
// message-legal aggregate wider than one slot is out of scope for this
// hand-assembled dispatch; every `pub` actor method in today's whole
// corpus takes only scalar params, so this never silently narrows a real
// case).

/// plans/M10.md item 0b (decisions 554/567): one turn's index into the
/// single contiguous `RT.turns` array `place_runtime_tables` lays down at
/// `rtdata_base`. The whole point of item 0 — the value a waker, a
/// reply-ring slot or a group's `owner_turn` will carry once 0c1/0c2/0c3
/// land, in place of a raw turn-area address.
///
/// **1-based, deliberately.** Array index 0 is a live turn in every
/// actor-bearing image (`tables.actors[0]`'s turn is placed first), so `0`
/// is not a free value — it is *made* free by biasing the id, and it then
/// serves as the `Option[TurnId]` niche. That is already the machine's
/// convention for exactly this problem: a group id is `arena_index + 1`
/// with `0` meaning "no ambient group". The field is private and
/// `from_index` is the only constructor, so `TurnId(0)` is
/// unconstructible — this type is the one place the niche is enforced,
/// rather than nine `cbz` sites assuming it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct TurnId(u32);

impl TurnId {
    /// The id of turn-array element `index` (0-based, `place_runtime_tables`'
    /// own order: actors, then messageable drivers, then free turns).
    pub fn from_index(index: usize) -> TurnId {
        // `n_turns` is bounded by the declaration count of one image; a
        // u32 overflow here would mean a 4-billion-root image, which the
        // 1 GiB ceiling refuses long before this does.
        let biased = u32::try_from(index + 1)
            .expect("a turn array with over 4 billion entries cannot fit the machine's memory");
        TurnId(biased)
    }

    /// The 1-based value an `Option[TurnId]` word holds — never `0`, which
    /// is what makes every existing `cbz`/`str xzr` "no waker" test keep
    /// meaning "none".
    pub fn get(self) -> u32 {
        debug_assert!(self.0 != 0, "TurnId(0) is the None niche, not an id");
        self.0
    }

    /// The 0-based array index — what `RuntimePlacement::turn_addr` scales
    /// by the stride. Bias removal lives here and nowhere else.
    pub fn index(self) -> usize {
        debug_assert!(self.0 != 0, "TurnId(0) is the None niche, not an id");
        self.0 as usize - 1
    }
}

/// plans/M10.md item E2 / decision 669: one group's index into the
/// group arena, encoded the way ambient lineage already encodes it —
/// `arena_index + 1`, with `0` meaning "no ambient group". That zero is
/// the `Option[GroupId]` None niche (decision 567's convention, already
/// the machine's). Same shape as [`TurnId`]: private field, `from_index`
/// only, so `GroupId(0)` is unconstructible in Rust.
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

/// One actor's own absolute runtime-table addresses, placed sequentially
/// from a given base (`rtdata_base` for a real image, or a host-mmap'd
/// stand-in base for a JIT/HVF test) — the exact byte order
/// `compute_runtime_tables`'s own `RuntimeTables::total_bytes` already
/// accounts for (state, ring, head/tail/count), so a real image's `rtdata`
/// section and this fn's own addresses can never disagree.
#[derive(Debug, Clone, Copy)]
pub struct ActorAddrs {
    pub state: u64,
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
    /// This actor's own turn area base — no longer bumped alongside the
    /// four fields above (plans/M10.md item 0b): it is
    /// `turns_base + (this actor's TurnId index << log2 stride)`, an
    /// element of the one contiguous turn array. Still a build-time
    /// absolute address, because the fully-unrolled scans want one.
    ///
    /// The fixed 48-byte turn record
    /// (`codegen::OFF_TURN_*`: busy/suspended/resume_ready/reply/waker/
    /// cur_method) followed by its persistent async frame slots. The
    /// address every message this actor's turns *send* carries as their
    /// waker, and the address `Reloc::TurnFrameAddr` resolves to for its
    /// own async methods.
    pub turn: u64,
}

impl ActorAddrs {
    /// This actor's mailbox, as the plain bounded ring it is — the shape
    /// `build_ring_enqueue` produces into, shared verbatim with every
    /// cross-core ring (plans/M8.md item C2, decision 28).
    pub fn mailbox(&self) -> RingAddrs {
        RingAddrs {
            ring: self.ring,
            head: self.head,
            tail: self.tail,
            count: self.count,
        }
    }
}

/// Every runtime-table address, placed from one `base` (`rtdata_base` for
/// a real image, a host-mmap'd stand-in for a JIT/HVF test) in the exact
/// byte order `compute_runtime_tables::total_bytes` accounts for.
///
/// plans/M10.md item 0b (decision 554) re-grouped that order. It is now:
/// the whole **turn array** first (`n_turns * turn_stride`, actors then
/// messageable drivers then free turns), then each actor's region (state,
/// ring, head/tail/count — the turn area no longer rides along), then each
/// driver's state and, when messageable, its own ring/head/tail/count, then
/// the per-core scheduler stripe, the group arena, and the cross-core
/// rings.
///
/// Turns first buys the one property `TurnId` needs: `turns_base ==
/// rtdata_base` exactly, so the address the whole runtime indexes from *is*
/// a section base and needs no arithmetic to resolve. Keeping them last
/// instead would only have preserved goldens this item moves anyway.
#[derive(Debug, Clone, Default)]
pub struct RuntimePlacement {
    /// plans/M10.md item 0b: the base of the one contiguous turn array —
    /// equal to `base` itself, and so to `rtdata_base` for a real image.
    pub turns_base: u64,
    /// The uniform stride the array is indexed at, copied from
    /// `RuntimeTables::turn_stride` so `turn_addr` below needs no second
    /// argument (`0` for an image with no turns, which then never indexes).
    pub turn_stride: u64,
    /// plans/M10.md item 0b: free async fn key -> that fn's own `TurnId` —
    /// the exact twin of `free_turns` below, and the only owner kind whose
    /// id is not positional (an actor's is its `tables.actors` index; a
    /// messageable driver's is `actors.len()` + its rank among the
    /// messageable drivers). `turn_id_for` is the reader; nothing indexes
    /// this map directly.
    pub turn_ids: BTreeMap<String, TurnId>,
    pub actors: Vec<ActorAddrs>,
    /// plans/M7.md item H1: each declared `@driver` instance's own state
    /// address, in `RuntimeTables::drivers` order. Placed after every
    /// actor's region and before the free-turn areas.
    ///
    /// plans/M8.md item D: a messageable driver's region continues past
    /// its state with the same ring/head/tail/count/turn run an actor's
    /// does, in the same order `RuntimeTables::total_bytes` accounts for —
    /// `driver_mailboxes` below holds those addresses, keyed by the same
    /// index. The `state` word here is unchanged either way.
    ///
    /// M12 item D: sticky wake-pending bits are no longer a trail word of
    /// driver state — they live at `wake_base` (contiguous `WAKE` array).
    pub drivers: Vec<u64>,
    /// plans/M8.md item D: `RuntimeTables::drivers` index -> that
    /// messageable driver's own mailbox addresses. An `ActorAddrs`
    /// deliberately, not a driver-shaped twin: it is handed straight to
    /// `build_rt_enqueue` / `build_rt_select_and_run_symbolic`, the same
    /// two routines every actor's mailbox is built from. Its `state` field
    /// is the same address `drivers[i]` holds.
    pub driver_mailboxes: BTreeMap<usize, ActorAddrs>,
    /// fn key -> free-turn area base (`RuntimeTables::free_turns` order).
    pub free_turns: BTreeMap<String, u64>,
    /// The deterministic round-robin cursor word each core's own
    /// `rt_run_one` reads/advances (04 §2's tie-breaker; at M6 every
    /// scheduling key is equal, so the cursor is the whole selection order
    /// among that core's ready actors). One per live core
    /// (`RuntimeTables::cores`) — 04 §2's event loop is per core, so its
    /// cursor is too; a shared cursor would make one core's selection
    /// depend on another's, which is exactly the migration/stealing the
    /// chapter forbids. `rr_cursors[0]` is core 0's, at the identical
    /// address the single global cursor occupied before item C1.
    pub rr_cursors: Vec<u64>,
    /// plans/M6.md item F: the whole-image group arena's own base address
    /// — `Reloc::GroupArenaBase`'s own resolution target, placed last
    /// (`RuntimeTables::total_bytes`'s own byte-order doc: actors, free
    /// turns, ready-queue table, rr cursor, then the group arena).
    pub group_arena: u64,
    /// plans/M8.md item C2: each cross-core ring's own placed addresses, in
    /// `RuntimeTables::rings` order — placed after the group arena, so an
    /// image with no cross-core edge places byte-for-byte what it did
    /// before this item.
    pub rings: Vec<RingAddrs>,
    /// M12 item D (decisions 880–882): base of the contiguous
    /// `WAKE.wake_pending` array, placed after rings. Equal to the rings
    /// end when `wake_pending_addrs` is empty (no reservation).
    pub wake_base: u64,
}

impl RuntimePlacement {
    /// The address of turn-array element `id` — the one index→address rule,
    /// `turns_base + (index << log2 stride)`. `place_runtime_tables` fills
    /// every `ActorAddrs::turn` / `free_turns` value from this same
    /// expression, so a build-time address and a `TurnId` can never name
    /// different bytes.
    pub fn turn_addr(&self, id: TurnId) -> u64 {
        self.turns_base + (id.index() as u64) * self.turn_stride
    }

    /// `log2(turn_stride)` — the shift the index→address rule scales an
    /// index by, and the whole reason item 0a made the stride a power of
    /// two. `0` for an image with no turns, which then never indexes (no
    /// `rt_*` routine is emitted at all). (The one live emitter of the
    /// rule, `codegen::push_turn_addr_from_id`, multiplies by a relocated
    /// stride instead — see its doc; the dead harness twin that used this
    /// shift was deleted in M10 item M, sweep find L-11.)
    pub fn log2_turn_stride(&self) -> u8 {
        if self.turn_stride == 0 {
            0
        } else {
            debug_assert!(self.turn_stride.is_power_of_two());
            self.turn_stride.trailing_zeros() as u8
        }
    }

    /// The `TurnId` of async fn `key`'s turn (`turn_owner`'s own rule): an
    /// actor method's turn is its actor's; a messageable driver's `pub async
    /// fn` parks in the driver's one turn (plans/M8.md item D —
    /// non-reentrancy is per root, not per method); anything else owns its
    /// own free turn. `None` only for a key the tables never sized — an
    /// internal inconsistency the caller reports loudly.
    ///
    /// plans/M10.md item 0b: this is *the* owner-resolution rule.
    /// `turn_area_for` below is defined in terms of it rather than beside
    /// it, so there is one rule and not two that could skew.
    pub fn turn_id_for(&self, key: &str, tables: &RuntimeTables) -> Option<TurnId> {
        let roots = mailbox_root_names(tables);
        match turn_owner(key, &roots) {
            Some(root) => {
                if let Some(i) = tables.actors.iter().position(|a| a.name == root) {
                    return Some(TurnId::from_index(i));
                }
                let di = tables.drivers.iter().position(|d| d.name == root)?;
                // `driver_mailboxes` is keyed by `tables.drivers` index and
                // is a `BTreeMap`, so its key order *is* the messageable
                // subsequence of `tables.drivers` — the same order
                // `place_runtime_tables` assigned indices in.
                let rank = self.driver_mailboxes.keys().position(|k| *k == di)?;
                Some(TurnId::from_index(tables.actors.len() + rank))
            }
            None => self.turn_ids.get(key).copied(),
        }
    }

    /// The turn area address for async fn `key` — `turn_id_for` scaled by
    /// the stride.
    pub fn turn_area_for(&self, key: &str, tables: &RuntimeTables) -> Option<u64> {
        self.turn_id_for(key, tables).map(|id| self.turn_addr(id))
    }
}

/// plans/M6.md decision 11b (02-language.md §12.2): resolves every
/// runtime test's own declared `Actor[T]` params against the image
/// graph's own declared instances — `T`'s *unique* instance across
/// `graph.actors`/`graph.drivers` (both are actor roots, 02 §9.1: "A
/// struct marked `@actor` ... or `@driver` ... is an actor"), by build-
/// time index (04-compiler.md §6's own "Actor as-if" license: a handle's
/// runtime value is just that instance's own build-time-constant index).
/// Zero or more than one instance is a named `error[build]` line listing
/// every candidate (`actor#i`/`driver#i`, the identical spelling
/// `eval::image::dump`'s own edge lines already use) — sema
/// (`check_runtime_test_params`) already guarantees every param here is
/// a plain `Actor[T]` handle, so the only failure mode left is a real
/// ambiguity/absence in *this* image. A test with no params (every sync
/// test, and any async test that declares none) resolves to an empty arg
/// list — byte-identical to every pre-decision-11b test.
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
            // plans/M8.md item D: a `@driver` declared with `mailbox=` is a
            // messageable actor root, so a runtime test may hold its handle
            // like any other. A driver *without* one still resolves as a
            // candidate — that is how the count check above stays honest —
            // but produces the named floor below rather than an index.
            // plans/M8.md item H attack 6: the handle word shares one index
            // space with every other `ImageDecl` (`image_decl_handle_word`).
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
