//! VMM/report line emitters for image layout (`append_*_vmm_lines`,
//! `render_layout_section`).

use std::collections::BTreeMap;

use wrela_machine::{console, layout as machine_layout};

use crate::eval::image::ImageGraph;
use crate::eval::image::push_line;

use super::place::place_runtime_tables;
use super::{
    BlkQueueReport, BlkReport, ImageLayout, LayoutError, RuntimeTables, Section, derive_blk_report,
    mailbox_root_names, rings_reservation_bytes, verify_ring_windows,
};

pub fn append_vmm_runtime_lines(out: &mut String, layout: &ImageLayout) {
    // Build the VMM-overlapping structured subset, then render Kind lines
    // through `wrela_machine::report` so emitter and parser share spellings.
    // Ring lines stay local: `RequestRing` is lossy vs live `Ring kind=…`.
    let parsed = parsed_runtime_tail(layout);
    out.push_str(&wrela_machine::report::line_cores(parsed.cores));
    out.push('\n');
    for s in &parsed.core_stacks {
        out.push_str(&wrela_machine::report::line_core_stack(
            s.core, s.base, s.size,
        ));
        out.push('\n');
    }
    for e in &parsed.core_entries {
        out.push_str(&wrela_machine::report::line_core_entry(e.core, e.base));
        out.push('\n');
    }
    append_ring_vmm_lines(out, layout);
    if let Some(blk) = &parsed.blk {
        out.push_str(&wrela_machine::report::line_blk_device(blk));
        out.push('\n');
        out.push_str(&wrela_machine::report::line_blk_queue(&blk.queue));
        out.push('\n');
        for p in &blk.pools {
            out.push_str(&wrela_machine::report::line_blk_pool(p));
            out.push('\n');
        }
        // BlkAccounting is compiler-emitted (not in ParsedReport); keep it.
        if let Some(rep) = &layout.blk {
            out.push_str(&fmt_blk_accounting(
                rep.descriptors_per_op,
                rep.occupancy_bound,
                None,
            ));
            out.push('\n');
        }
    }
    for inj in &parsed.irq_injects {
        out.push_str(&wrela_machine::report::line_irq_host_inject(
            inj.base, inj.offset, inj.status, inj.vector,
        ));
        out.push('\n');
    }
}

/// VMM-overlapping runtime facts from an `ImageLayout`, as
/// `wrela_machine::report::ParsedReport` fields (identity/sections left
/// empty — callers that need a full boot report fill those and call
/// `wrela_machine::report::render`).
pub fn parsed_runtime_tail(layout: &ImageLayout) -> wrela_machine::report::ParsedReport {
    use wrela_machine::report::{
        BlkConfig, BlkQueueConfig, CoreEntry, IrqHostInject, ParsedReport, PoolWindow,
    };
    let core_entries = layout
        .core_entries
        .iter()
        .map(|&(core, base)| CoreEntry { core, base })
        .collect();
    let irq_injects = layout
        .irq_host_injects
        .iter()
        .map(|inj| IrqHostInject {
            base: inj.base,
            offset: inj.offset,
            status: inj.status,
            vector: inj.vector,
        })
        .collect();
    let blk = layout.blk.as_ref().map(|b| {
        let mut pools = Vec::new();
        for p in &layout.pools {
            let Some(dev) = p.backing.device else {
                continue;
            };
            pools.push(PoolWindow {
                name: p.backing.name.clone(),
                device: dev as u64,
                base: p.base,
                size: p.backing.bytes,
            });
        }
        BlkConfig {
            device: b.device as u64,
            capacity_sectors: b.capacity_sectors,
            features: b.features,
            vector: b.vector,
            queue: BlkQueueConfig {
                size: b.queue.size,
                desc: b.queue.desc,
                avail: b.queue.avail,
                used: b.queue.used,
                doorbell: b.queue.doorbell,
            },
            pools,
        }
    });
    let cores = layout.cores.max(1);
    let core_stacks = (0..cores)
        .map(|core| wrela_machine::report::CoreStack {
            core,
            base: machine_layout::core_stack_base_n(core, cores),
            size: machine_layout::CORE_STACK_SIZE,
        })
        .collect();
    ParsedReport {
        entry: 0,
        image_sha256: String::new(),
        input_digests: Vec::new(),
        exec_sections: Vec::new(),
        blk,
        irq_injects,
        core_entries,
        cores,
        core_stacks,
        request_rings: Vec::new(),
    }
}

/// The `Ring ...` lines, in `RuntimeTables::rings` order — the same order
/// `build_rt_drain` walks its lanes, which is what makes the VMM's
/// reconstruction of admission order an ordered one. Shares
/// `render_layout_section`'s own rendering so the runtime report and the
/// `--stage=report` artifact cannot disagree about a ring.
pub fn append_ring_vmm_lines(out: &mut String, layout: &ImageLayout) {
    for line in ring_report_lines(layout) {
        out.push_str(&line);
        out.push('\n');
    }
    // plans/M12.md item C: uniform-stride summary — the VMM needs
    // `stride` to derive CTL addresses and overlap spans under
    // CTL-then-DATA packing. Absent when this image has no rings.
    if let Some(tables) = &layout.runtime {
        if !tables.rings.is_empty() {
            out.push_str(&format!(
                "Rings count={} stride={} padding={} bytes={}\n",
                tables.rings.len(),
                tables.ring_stride,
                tables.rings_padding,
                rings_reservation_bytes(&tables.rings)
            ));
        }
    }
}

/// One `Ring kind=... base=0x...` line per cross-core ring. Empty for an
/// image with no cross-core message edge. The addresses are recomputed
/// from the already-placed `rtdata` base through the identical
/// `place_runtime_tables` the emitter used, never by a second rule that
/// could drift from it.
pub(crate) fn ring_report_lines(layout: &ImageLayout) -> Vec<String> {
    let Some(tables) = &layout.runtime else {
        return Vec::new();
    };
    if tables.rings.is_empty() {
        return Vec::new();
    }
    let rtdata_base = layout
        .sections
        .iter()
        .find(|s| s.name == "rtdata")
        .map(|s| s.base)
        .unwrap_or_else(|| {
            debug_assert!(false, "an image with rings always places `rtdata`");
            0
        });
    let addrs = place_runtime_tables(rtdata_base, tables).rings;
    tables
        .rings
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "Ring kind={} src={} dst={} target={} cap={} slot={} bytes={} base={:#x}",
                r.kind_name(),
                r.src,
                r.dst,
                r.actor.as_deref().unwrap_or("-"),
                r.capacity,
                r.slot_size,
                r.bytes(),
                addrs[i].ring,
            )
        })
        .collect()
}

/// Shared `BlkDevice ...` line body (no trailing newline).
pub(crate) fn fmt_blk_device(blk: &BlkReport) -> String {
    format!(
        "BlkDevice device=device#{} capacity_sectors={} features={:#x}{}",
        blk.device,
        blk.capacity_sectors,
        blk.features,
        match blk.vector {
            Some(v) => format!(" vector={v}"),
            None => String::new(),
        }
    )
}

/// Shared `BlkQueue ...` line body (no trailing newline).
pub(crate) fn fmt_blk_queue(q: &BlkQueueReport) -> String {
    format!(
        "BlkQueue index={} size={} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}",
        q.index, q.size, q.desc, q.avail, q.used, q.doorbell
    )
}

/// Shared `BlkPool ...` line body (no trailing newline).
pub(crate) fn fmt_blk_pool(name: &str, device: usize, base: u64, size: u64) -> String {
    format!("BlkPool name={name} device=device#{device} base={base:#x} size={size:#x}")
}

/// Shared `BlkAccounting ...` line body. The VMM hand-built path omits
/// `queue_depth=`; the full report includes it.
pub(crate) fn fmt_blk_accounting(
    descriptors_per_op: u16,
    occupancy_bound: u16,
    queue_depth: Option<u16>,
) -> String {
    match queue_depth {
        Some(qd) => format!(
            "BlkAccounting descriptors_per_op={descriptors_per_op} queue_depth={qd} \
             occupancy_bound={occupancy_bound}"
        ),
        None => format!(
            "BlkAccounting descriptors_per_op={descriptors_per_op} occupancy_bound={occupancy_bound}"
        ),
    }
}

/// Append the VMM-facing `BlkDevice`/`BlkQueue`/`BlkPool` lines (and the
/// decision-2c accounting fact E1 can honestly derive) for a test-image
/// hand-built report. No-op when `layout.blk` is `None`.
pub fn append_blk_vmm_lines(out: &mut String, layout: &ImageLayout) {
    let Some(blk) = &layout.blk else {
        return;
    };
    out.push_str(&fmt_blk_device(blk));
    out.push('\n');
    out.push_str(&fmt_blk_queue(&blk.queue));
    out.push('\n');
    // Every device-reachable pool — ring *and* payload — matching the
    // full `--stage=report` `BlkPool` set (decision 5: the VMM maps exactly
    // these windows). Emitting only the queue's control pool left payload
    // DMA unmapped and failed the flagship write at the first descriptor.
    //
    // plans/M8.md item P: each line carries the device it is bound to, and
    // the set is still *every* device-reachable pool, not just this
    // device's — the VMM needs to know a window exists in order to refuse
    // it to a device that does not own it.
    for p in &layout.pools {
        let Some(dev) = p.backing.device else {
            continue;
        };
        out.push_str(&fmt_blk_pool(&p.backing.name, dev, p.base, p.backing.bytes));
        out.push('\n');
    }
    out.push_str(&fmt_blk_accounting(
        blk.descriptors_per_op,
        blk.occupancy_bound,
        None,
    ));
    out.push('\n');
}

/// Fill `layout.blk` from configure sites + placed pools, then re-verify
/// ring windows. Called by every consumer that has `programs` after layout.
pub fn attach_blk_report(
    layout: &mut ImageLayout,
    graph: &ImageGraph,
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<(), LayoutError> {
    layout.blk = derive_blk_report(&layout.pools, graph, programs)?;
    verify_ring_windows(&layout.pools, &layout.blk)
}

// --- report rendering (decision 7's own Layout section) -------------------

/// The fixed machine region below `IMAGE_BASE`: machine-info page plus
/// console ring/data pages, combined into one `pages` fact. Live stacks
/// are report-owned in high DRAM (`Cores` / `CoreStack`, plans/M15.md
/// item D) — the former low `stacks` section is omitted.
fn pages_region() -> (u64, u64) {
    let base = machine_layout::MACHINE_INFO_BASE;
    let end = console::DATA_BASE + console::DATA_SIZE;
    (base, end - base)
}

/// Appends the `Layout` section (decision 7) to an already-rendered report
/// buffer: `pages`, sealed `Cores`/`CoreStack` (high DRAM), every section
/// `layout` actually placed (`entry`/`code`/`rodata`?/`abort`), then the
/// separate `Entry base=0x...` fact.
pub fn render_layout_section(out: &mut String, layout: &ImageLayout) {
    let (pages_base, pages_size) = pages_region();
    push_line(
        out,
        1,
        &format!("Section name=pages base={pages_base:#x} size={pages_size}"),
    );
    let n_cores = layout.cores.max(1);
    push_line(out, 1, &wrela_machine::report::line_cores(n_cores));
    for core in 0..n_cores {
        let base = machine_layout::core_stack_base_n(core, n_cores);
        push_line(
            out,
            1,
            &wrela_machine::report::line_core_stack(core, base, machine_layout::CORE_STACK_SIZE),
        );
    }
    for s in &layout.sections {
        push_line(
            out,
            1,
            &format!("Section name={} base={:#x} size={}", s.name, s.base, s.size),
        );
    }
    push_line(out, 1, &format!("Entry base={:#x}", layout.entry));
    // plans/M8.md item C1: one line per secondary core this image brings
    // up — the address the VMM starts that vCPU at once core 0 rings
    // `mmio::RELEASE_MMIO_ADDR` (06 §3: "releases the other vCPUs").
    // Absent entirely for a single-core image, so no pre-C1 report golden
    // moves; core 0's own entry stays the `Entry base=` line above.
    for (core, base) in &layout.core_entries {
        push_line(out, 1, &format!("CoreEntry core={core} base={base:#x}"));
    }

    // plans/M10.md item A2c / decision 588: one line per `@placed` static.
    // Absent entirely when the image declares none, so no pre-A2c report
    // golden moves.
    for s in &layout.placed_statics {
        push_line(
            out,
            1,
            &format!(
                "PlacedStatic name={} type={} addr={:#x} size={}",
                s.name, s.ty, s.addr, s.size
            ),
        );
    }
    // plans/M12.md item G / decisions 890–893: census line after the
    // per-static list. `spans` is live `N_INIT_SLOTS` (not the INIT_SPAN
    // placeholder pool); `count` excludes high-zone INIT_SPAN placeholders
    // so the ratchet `N ≤ FIXED_SET_LEN + spans` stays honest while
    // `runtime.wr` still imports INIT_SPAN0..7. Absent when no placed
    // statics (same no-placeholder rule as the lines above).
    if !layout.placed_statics.is_empty() {
        let spans = layout
            .runtime
            .as_ref()
            .map(|t| {
                crate::rtconfig::live_init_span_count(t)
                    .expect("sealed ImageLayout runtime tables agree with placement")
            })
            .unwrap_or(0);
        let census = crate::placed_static_census::summarize(&layout.placed_statics, spans);
        push_line(out, 1, &census.render_line());
    }

    // --- plans/M6.md item C, decision 3: per-actor runtime-table
    // accounting — facts only, absent entirely when this image has no
    // actors (`ImageLayout::runtime`'s own doc comment: never a
    // placeholder). Appended after `Entry base=...` (04-compiler.md §7's
    // own "this milestone appends sections without reshuffling" reading,
    // mirrored here): every existing report golden's `Layout` section
    // text up to and including its own `Entry base=...` line stays
    // byte-identical; only actor-bearing images gain anything past it.
    if let Some(tables) = &layout.runtime {
        for a in &tables.actors {
            push_line(
                out,
                1,
                &format!(
                    "Actor name={} mailbox={} slot={} frame={} state={}",
                    a.name, a.mailbox_capacity, a.slot_size, a.frame_size, a.state_size
                ),
            );
        }
        // Free-turn areas (park-and-resume: one per non-actor-owned
        // async fn — `RuntimeTables::free_turns`'s own doc comment) —
        // facts only, absent when no free async fn exists.
        for (key, area) in &tables.free_turns {
            push_line(out, 1, &format!("Turn fn={key} frame={area}"));
        }
        // plans/M7.md item H1: one line per declared `@driver` instance.
        // Deliberately its own line kind, not an `Actor ...` line with
        // blanks: a driver without `mailbox=` has no ring and no turn
        // area, and printing `mailbox=0 slot=0 frame=0` would read as
        // three decisions the image did not make.
        // plans/M8.md item D: a driver declared with `mailbox=` gains the
        // same three facts, appended to its own line — so the report says
        // which drivers are messageable and how much they cost, and a
        // driver without a mailbox still says nothing it does not have.
        for d in &tables.drivers {
            let mailbox = match &d.mailbox {
                None => String::new(),
                Some(mb) => format!(
                    " mailbox={} slot={} frame={}",
                    mb.capacity, mb.slot_size, mb.frame_size
                ),
            };
            push_line(
                out,
                1,
                &format!("Driver name={} state={}{mailbox}", d.name, d.state_size),
            );
        }
        // plans/M8.md item C2: this image's own cross-core SPSC rings —
        // 04 §3's "sealed graph" made visible, one line per ring, so a
        // reviewer can see how many there are, which core produces into
        // each, how deep it is, and where the capacity came from. Absent
        // entirely for an image with no cross-core message edge.
        //
        // plans/M8.md item C3, decision 42: the line also carries the
        // ring's own placed `base=`, because the report is the VMM's whole
        // configuration and 06 §8 makes the VMM the recorder of
        // "per-mailbox cross-core admission order" — an admission this VMM
        // cannot address is one it cannot witness. One renderer
        // (`ring_report_lines`) serves this artifact and the runtime
        // report the VMM actually parses, so the two cannot disagree.
        for line in ring_report_lines(layout) {
            push_line(out, 1, &line);
        }
        // plans/M10.md item 0b (decision 555): the turn array's own three
        // facts, published so an image can see — and later assert — what
        // the uniform stride costs it. Placed directly under the per-owner
        // `frame=` lines it summarizes, so a reviewer reads `frame=56 /
        // frame=472` and then `stride=1024` on adjacent lines and can take
        // the padding off the page; `Totals` stays last.
        //
        // Deliberately *not* folded into `Totals`: that would churn a line
        // reviewers skim, in every actor-bearing golden, for a reason
        // unrelated to totals. Absent entirely when this image has no
        // runtime table, like every other line in this block.
        //
        // Making these two numbers `@layout_assert`-able is a separate item
        // (decision 568): `ImageReport` is a closed eight-field stdlib
        // surface, and a stdlib change does not belong in a byte-identity
        // commit.
        push_line(
            out,
            1,
            &format!(
                "Turns count={} stride={} bytes={}",
                tables.n_turns,
                tables.turn_stride,
                tables.n_turns * tables.turn_stride
            ),
        );
        // plans/M12.md item C (decision 875): uniform ring-stride cost,
        // printed so a reviewer can see the padding trade. Absent when
        // this image has no cross-core rings (like the `Ring kind=` lines).
        if !tables.rings.is_empty() {
            push_line(
                out,
                1,
                &format!(
                    "Rings count={} stride={} padding={} bytes={}",
                    tables.rings.len(),
                    tables.ring_stride,
                    tables.rings_padding,
                    rings_reservation_bytes(&tables.rings)
                ),
            );
        }
        push_line(
            out,
            1,
            &format!(
                "Totals actors={} drivers={} ready_queue={} group_arena={} bytes={}",
                tables.actors.len(),
                tables.drivers.len(),
                tables.ready_queue_capacity,
                tables.group_arena_capacity,
                tables.total_bytes
            ),
        );
    }

    // --- plans/M7.md item H1: the device register windows, appended for
    // the identical reason every accounting block above it is — facts
    // only, absent entirely for an image that binds no driver.
    //
    // **One** line kind, deliberately — unlike item D's pool block, which
    // emits an accounting `Pool ...` line *and* a mapping `BlkPool ...`
    // line. A mapping line exists to be parsed by a device model, and
    // `wrela-vmm::parse_report` has no register-window field to parse into:
    // machine v1's virtio-blk model has no register file at all
    // (06-machine.md §3, `wrela-vmm`'s `devices` module doc). Inventing a
    // device-kind-prefixed second line now would be naming a protocol
    // against a consumer that does not exist; `DeviceRegs` carries every
    // fact such a consumer would need (base, size, which device, which
    // driver), and item G — which gives the window's other side a writer,
    // 03 §6's own `interrupt_status` — is where the mapping half lands.
    for r in &layout.device_regs {
        push_line(
            out,
            1,
            &format!(
                "DeviceRegs device=device#{} type={} driver={} base={:#x} size={}",
                r.device, r.device_type, r.driver, r.base, r.size
            ),
        );
    }
    // plans/M7.md item G: the mapping half of DeviceRegs — a host write
    // into `interrupt_status` plus the vector to raise. Parsed by the
    // VMM (`IrqHostInject`); absent when no ISR is bound.
    for inj in &layout.irq_host_injects {
        push_line(
            out,
            1,
            &format!(
                "IrqHostInject base={:#x} offset={:#x} status={:#x} vector={}",
                inj.base, inj.offset, inj.status, inj.vector
            ),
        );
    }

    // --- plans/M7.md item D: DMA pool accounting (a milestone exit
    // criterion), appended after the actor tables for the identical
    // reason those were appended after `Entry base=...`: facts only,
    // absent entirely for an image with no pool, so no existing golden
    // moves that did not gain a pool.
    //
    // Two line kinds, and the split is the point:
    //
    // - `Pool ...` is the *accounting* line: 03-hardware.md §3's five
    //   declared facts about every bound pool, device-reachable or not
    //   (`PoolBacking`'s own doc comment derives each one). It is a
    //   report fact; nothing consumes it as configuration.
    // - `BlkPool name= device= base= size=` is the *mapping* line, and it
    //   exists for device-reachable pools only. It is the exact format
    //   `wrela-vmm`'s own `parse_report` reads (plans/M7.md item F,
    //   plans/M8.md item P), and the list of them is the whole of what
    //   that VMM maps — decision 5's security property, in the artifact
    //   rather than in a comment: a pool with no `device=` never produces
    //   one, so no device can reach it.
    //
    //   plans/M8.md item P made the line carry **its own device**, which
    //   is what the property needed all along: 03-hardware.md §3's "all
    //   memory a device can reach originates from *its* bound pools" is
    //   per-device, and until this landed the VMM handed every window to
    //   its one device model. The set emitted here is still every
    //   device-reachable pool in the image, not just the modelled
    //   device's — the VMM must know a window exists in order to refuse
    //   it to a device that does not own it, which is exactly what
    //   `golden/err-boot-blk-cross-device-pool` proves at boot.
    //
    // An image that declares a device-reachable pool but no queue is not
    // bootable yet, by design: `parse_report` refuses a `BlkPool` line
    // with no `BlkDevice`/`BlkQueue` to bind it to (those lines come from
    // item E1 when a configure site exists). Fail-closed and named,
    // rather than a window mapped for a device model that was never
    // configured.
    for p in &layout.pools {
        let b = &p.backing;
        let kind = if b.is_dma { "dma" } else { "image" };
        let mut line = format!(
            "Pool name={} kind={} payload={} slots={} slot_bytes={} base={:#x} size={} align={} \
             coherency=coherent",
            b.name, kind, b.payload, b.slots, b.slot_bytes, p.base, b.bytes, b.align
        );
        match b.device {
            Some(i) => line.push_str(&format!(" device=device#{i}")),
            None => line.push_str(" device=none"),
        }
        push_line(out, 1, &line);
    }
    for p in &layout.pools {
        let Some(dev) = p.backing.device else {
            continue;
        };
        push_line(
            out,
            1,
            &fmt_blk_pool(&p.backing.name, dev, p.base, p.backing.bytes),
        );
    }
    // plans/M7.md item E1: the VMM-facing device/queue lines
    // `parse_report` already consumes. Absent entirely until a
    // `VirtQueue.configure` site exists (`layout.blk`).
    if let Some(blk) = &layout.blk {
        push_line(out, 1, &fmt_blk_device(blk));
        let q = &blk.queue;
        push_line(out, 1, &fmt_blk_queue(q));
        // Decision 2c / plans/M7.md item E2: occupancy bound is
        // floor(queue_depth / descriptors_per_op). Exits-per-op stays
        // deferred (decision 21).
        push_line(
            out,
            1,
            &fmt_blk_accounting(blk.descriptors_per_op, blk.occupancy_bound, Some(q.size)),
        );
    }
}
