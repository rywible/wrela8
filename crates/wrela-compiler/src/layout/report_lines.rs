use std::collections::BTreeMap;

use wrela_machine::{console, layout as machine_layout};

use crate::eval::image::ImageGraph;
use crate::eval::image::push_line;

use super::place::place_runtime_tables;
use super::{
    BlkQueueReport, BlkReport, ImageLayout, LayoutError, derive_blk_report,
    rings_reservation_bytes, verify_ring_windows,
};

pub fn append_vmm_runtime_lines(out: &mut String, layout: &ImageLayout) {
    let parsed = parsed_runtime_tail(layout);
    for renderer in &layout.renderers {
        out.push_str(&format!(
            "RendererPlacement index={} frameprog_base={:#x} frameprog_bytes={} state_base={:#x} state_bytes={} coordinator={} coordinator_core={}\n",
            renderer.index,
            renderer.frameprog_base,
            renderer.frameprog_size,
            renderer.state_base,
            renderer.state_size,
            renderer.coordinator_actor,
            renderer.coordinator_core,
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_vmm_tail_includes_renderer_frame_program_placements() {
        let layout = ImageLayout {
            blob: Vec::new(),
            linked: None,
            entry: 0,
            sections: vec![super::super::Section {
                name: "frameprog",
                base: 0x40_1000,
                size: 256,
            }],
            runtime: None,
            pools: Vec::new(),
            device_regs: Vec::new(),
            blk: None,
            irq_host_injects: Vec::new(),
            core_entries: Vec::new(),
            cores: 1,
            placed_statics: Vec::new(),
            renderers: vec![super::super::RendererPlacement {
                index: 0,
                frameprog_base: 0x40_1000,
                frameprog_size: 256,
                state_base: 0,
                state_size: 0,
                coordinator_actor: String::new(),
                coordinator_core: 0,
                per_core: Vec::new(),
                framebuffer_base: 0,
                framebuffer_bytes: 0,
                probe_base: 0,
                probe_bytes: 0,
            }],
        };
        let mut report = String::new();
        append_vmm_runtime_lines(&mut report, &layout);
        assert!(report.starts_with(
            "RendererPlacement index=0 frameprog_base=0x401000 frameprog_bytes=256 state_base=0x0 state_bytes=0 coordinator= coordinator_core=0\n"
        ));
    }
}

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
        frameprog_sections: layout
            .sections
            .iter()
            .filter(|section| section.name == "frameprog")
            .map(|section| wrela_machine::report::ReportSection {
                name: section.name.to_string(),
                base: section.base,
                size: section.size,
            })
            .collect(),
        renderer_placements: layout
            .renderers
            .iter()
            .map(|renderer| wrela_machine::report::ReportRendererPlacement {
                index: renderer.index,
                frameprog_base: renderer.frameprog_base,
                frameprog_size: renderer.frameprog_size,
                state_base: renderer.state_base,
                state_size: renderer.state_size,
            })
            .collect(),
        blk,
        irq_injects,
        core_entries,
        cores,
        core_stacks,
        request_rings: Vec::new(),
    }
}

pub fn append_ring_vmm_lines(out: &mut String, layout: &ImageLayout) {
    for line in ring_report_lines(layout) {
        out.push_str(&line);
        out.push('\n');
    }
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

pub(crate) fn fmt_blk_device(blk: &BlkReport) -> String {
    wrela_machine::report::blk_device_line(
        blk.device as u64,
        blk.capacity_sectors,
        blk.features,
        blk.vector,
    )
}

pub(crate) fn fmt_blk_queue(q: &BlkQueueReport) -> String {
    wrela_machine::report::blk_queue_line(q.index, q.size, q.desc, q.avail, q.used, q.doorbell)
}

pub(crate) fn fmt_blk_pool(name: &str, device: usize, base: u64, size: u64) -> String {
    wrela_machine::report::blk_pool_line(name, device as u64, base, size)
}

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

pub fn append_blk_vmm_lines(out: &mut String, layout: &ImageLayout) {
    let Some(blk) = &layout.blk else {
        return;
    };
    out.push_str(&fmt_blk_device(blk));
    out.push('\n');
    out.push_str(&fmt_blk_queue(&blk.queue));
    out.push('\n');
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

pub fn attach_blk_report(
    layout: &mut ImageLayout,
    graph: &ImageGraph,
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<(), LayoutError> {
    layout.blk = derive_blk_report(&layout.pools, graph, programs)?;
    verify_ring_windows(&layout.pools, &layout.blk)
}

fn pages_region() -> (u64, u64) {
    let base = machine_layout::MACHINE_INFO_BASE;
    let end = console::DATA_BASE + console::DATA_SIZE;
    (base, end - base)
}

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
    for renderer in &layout.renderers {
        push_line(
            out,
            1,
            &format!(
                "RendererPlacement index={} frameprog_base={:#x} frameprog_bytes={} \
                 state_base={:#x} state_bytes={} coordinator={} coordinator_core={}",
                renderer.index,
                renderer.frameprog_base,
                renderer.frameprog_size,
                renderer.state_base,
                renderer.state_size,
                renderer.coordinator_actor,
                renderer.coordinator_core,
            ),
        );
        for worker in &renderer.per_core {
            push_line(
                out,
                1,
                &format!(
                    "RendererWorker renderer={} index={} actor={} core={} tiles=[{},{}) \
                     workspace_base={:#x} workspace_bytes={}",
                    renderer.index,
                    worker.worker_index,
                    worker.actor,
                    worker.core,
                    worker.tiles_start,
                    worker.tiles_end,
                    worker.workspace_base,
                    worker.workspace_bytes,
                ),
            );
        }
        push_line(
            out,
            1,
            &format!(
                "RendererMemory renderer={} framebuffer_base={:#x} framebuffer_bytes={} \
                 probe_base={:#x} probe_bytes={}",
                renderer.index,
                renderer.framebuffer_base,
                renderer.framebuffer_bytes,
                renderer.probe_base,
                renderer.probe_bytes,
            ),
        );
    }
    push_line(out, 1, &format!("Entry base={:#x}", layout.entry));
    for (core, base) in &layout.core_entries {
        push_line(out, 1, &format!("CoreEntry core={core} base={base:#x}"));
    }

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
        for (key, area) in &tables.free_turns {
            push_line(out, 1, &format!("Turn fn={key} frame={area}"));
        }
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
        for line in ring_report_lines(layout) {
            push_line(out, 1, &line);
        }
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
    if let Some(blk) = &layout.blk {
        push_line(out, 1, &fmt_blk_device(blk));
        let q = &blk.queue;
        push_line(out, 1, &fmt_blk_queue(q));
        push_line(
            out,
            1,
            &fmt_blk_accounting(blk.descriptors_per_op, blk.occupancy_bound, Some(q.size)),
        );
    }
}
