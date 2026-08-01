use std::collections::BTreeMap;

use crate::eval::image::{DeclArg, ImageDeclRef, ImageGraph};
use crate::eval::value::Value;
use crate::mwir::{self, LayoutCtx};
use crate::sema::types::{self, Type};
use crate::syntax::ast::Module;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementSource {
    Explicit,
    PinnedVirtioBlk,
    Inferred,
}

impl PlacementSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PlacementSource::Explicit => "explicit",
            PlacementSource::PinnedVirtioBlk => "pinned-virtio-blk",
            PlacementSource::Inferred => "inferred",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementEntry {
    pub id: ImageDeclRef,
    pub type_name: String,
    pub core: usize,
    pub source: PlacementSource,
    pub work: u64,
    pub work_source: &'static str,
    pub bytes: u64,
    pub bytes_state: u64,
    pub bytes_mailbox: u64,
    pub bytes_pool: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementTable {
    pub entries: Vec<PlacementEntry>,
    pub cores: usize,
}

impl Default for PlacementTable {
    fn default() -> PlacementTable {
        PlacementTable {
            entries: Vec::new(),
            cores: 1,
        }
    }
}

impl PlacementTable {
    pub fn core_of(&self, id: &ImageDeclRef) -> Option<usize> {
        self.entries.iter().find(|e| e.id == *id).map(|e| e.core)
    }

    pub fn core_of_actor_type(&self, type_name: &str) -> Option<usize> {
        let mut found: Option<usize> = None;
        for e in self.entries.iter().filter(|e| e.type_name == type_name) {
            match found {
                None => found = Some(e.core),
                Some(c) if c == e.core => {}
                Some(_) => return None,
            }
        }
        found
    }
}

pub fn check_annotations(graph: &ImageGraph) -> Result<(), String> {
    let n = graph.cores;
    for (i, d) in graph.drivers.iter().enumerate() {
        let name = types::render_type(&d.actor_type);
        let id = format!("driver#{i}");
        match read_core_arg(&d.args, &id, n)? {
            Some(core) => {
                if driver_is_virtio_blk(graph, d) && core != 0 {
                    return Err(format!(
                        "`{id}` ({name}) is the virtio-blk driver; plans/M8.md keeps it and its \
                         ISR/bottom half on core 0 until an item deliberately moves them \
                         (refusing `core={core}`)"
                    ));
                }
            }
            None => {}
        }
    }
    for (i, d) in graph.actors.iter().enumerate() {
        let id = format!("actor#{i}");
        let _ = read_core_arg(&d.args, &id, n)?;
    }
    Ok(())
}

pub fn place(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
    cores: usize,
) -> Result<PlacementTable, String> {
    if cores != graph.cores {
        return Err(format!(
            "internal error: placement: cores={cores} disagrees with ImageGraph.cores={}",
            graph.cores
        ));
    }
    check_annotations(graph)?;

    let empty_frames = BTreeMap::new();
    let tables = crate::layout::compute_runtime_tables(
        graph,
        modules,
        layout_ctx,
        &empty_frames,
        crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
    )?
    .unwrap_or_default();

    let pool_by_driver = dma_pool_bytes_by_driver(graph, layout_ctx)?;

    let mut placeables: Vec<Placeable> = Vec::new();
    for (i, d) in graph.drivers.iter().enumerate() {
        let type_name = types::render_type(&d.actor_type);
        let state = tables.drivers.get(i).map(|r| r.state_size).unwrap_or(0);
        let mailbox = tables
            .drivers
            .get(i)
            .and_then(|r| r.mailbox.as_ref())
            .map(|m| m.capacity * m.slot_size + MAILBOX_BOOKKEEPING)
            .unwrap_or(0);
        let pool = pool_by_driver.get(&i).copied().unwrap_or(0);
        let explicit = read_core_arg(&d.args, &format!("driver#{i}"), cores)?;
        let pinned = driver_is_virtio_blk(graph, d);
        placeables.push(Placeable {
            id: ImageDeclRef::Driver(i),
            type_name,
            work: 0,
            bytes_state: state,
            bytes_mailbox: mailbox,
            bytes_pool: pool,
            explicit,
            pinned_virtio_blk: pinned,
        });
    }
    for (i, d) in graph.actors.iter().enumerate() {
        let type_name = types::render_type(&d.actor_type);
        let rt = tables.actors.get(i).ok_or_else(|| {
            format!(
                "internal error: placement: actor#{i} ({type_name}) missing from runtime tables"
            )
        })?;
        let bytes_mailbox = rt.mailbox_capacity * rt.slot_size + MAILBOX_BOOKKEEPING;
        let explicit = read_core_arg(&d.args, &format!("actor#{i}"), cores)?;
        placeables.push(Placeable {
            id: ImageDeclRef::Actor(i),
            type_name,
            work: 0,
            bytes_state: rt.state_size,
            bytes_mailbox,
            bytes_pool: 0,
            explicit,
            pinned_virtio_blk: false,
        });
    }

    let live_cores = cores;

    let mut core_load: Vec<(u64, u64)> = vec![(0, 0); live_cores];
    let mut assigned: BTreeMap<ImageDeclRef, (usize, PlacementSource)> = BTreeMap::new();

    for p in &placeables {
        if let Some(core) = p.explicit {
            add_load(&mut core_load, core, p.work, p.bytes());
            assigned.insert(p.id.clone(), (core, PlacementSource::Explicit));
        } else if p.pinned_virtio_blk {
            add_load(&mut core_load, 0, p.work, p.bytes());
            assigned.insert(p.id.clone(), (0, PlacementSource::PinnedVirtioBlk));
        }
    }

    let mut unfixed: Vec<&Placeable> = placeables
        .iter()
        .filter(|p| !assigned.contains_key(&p.id))
        .collect();
    unfixed.sort_by(|a, b| {
        b.work
            .cmp(&a.work)
            .then_with(|| b.bytes().cmp(&a.bytes()))
            .then_with(|| a.id.cmp(&b.id))
    });

    for p in unfixed {
        let core = pick_core(&core_load);
        add_load(&mut core_load, core, p.work, p.bytes());
        assigned.insert(p.id.clone(), (core, PlacementSource::Inferred));
    }

    let mut entries = Vec::with_capacity(placeables.len());
    for p in &placeables {
        let (core, source) = assigned.remove(&p.id).ok_or_else(|| {
            format!(
                "internal error: placement: {} was not assigned a core",
                p.id.render()
            )
        })?;
        entries.push(PlacementEntry {
            id: p.id.clone(),
            type_name: p.type_name.clone(),
            core,
            source,
            work: p.work,
            work_source: "unproved",
            bytes: p.bytes(),
            bytes_state: p.bytes_state,
            bytes_mailbox: p.bytes_mailbox,
            bytes_pool: p.bytes_pool,
        });
    }

    Ok(PlacementTable {
        entries,
        cores: live_cores,
    })
}

pub fn render_placement_section(out: &mut String, table: &PlacementTable) {
    for e in &table.entries {
        crate::eval::image::push_line(
            out,
            1,
            &format!(
                "Placement id={} type={} core={} source={} work={} work_source={} \
                 bytes={} bytes_state={} bytes_mailbox={} bytes_pool={}",
                e.id.render(),
                e.type_name,
                e.core,
                e.source.as_str(),
                e.work,
                e.work_source,
                e.bytes,
                e.bytes_state,
                e.bytes_mailbox,
                e.bytes_pool,
            ),
        );
    }
}

const MAILBOX_BOOKKEEPING: u64 = 3 * 8;

struct Placeable {
    id: ImageDeclRef,
    type_name: String,
    work: u64,
    bytes_state: u64,
    bytes_mailbox: u64,
    bytes_pool: u64,
    explicit: Option<usize>,
    pinned_virtio_blk: bool,
}

impl Placeable {
    fn bytes(&self) -> u64 {
        self.bytes_state + self.bytes_mailbox + self.bytes_pool
    }
}

fn add_load(loads: &mut [(u64, u64)], core: usize, work: u64, bytes: u64) {
    loads[core].0 += work;
    loads[core].1 += bytes;
}

fn pick_core(loads: &[(u64, u64)]) -> usize {
    let mut best = 0;
    for (i, load) in loads.iter().enumerate().skip(1) {
        if load < &loads[best] {
            best = i;
        }
    }
    best
}

fn read_core_arg(args: &[DeclArg], id: &str, cores: usize) -> Result<Option<usize>, String> {
    let Some(a) = args.iter().find(|a| a.label == "core") else {
        return Ok(None);
    };
    let Some(n) = value_as_u64(&a.value) else {
        return Err(format!(
            "`{id}` sets `core=...` to a value that is not a plain non-negative integer"
        ));
    };
    if n as usize >= cores {
        return Err(format!(
            "`{id}` sets `core={n}`, which is outside 0..{cores} (04-compiler.md §3: placement \
             domain is 0..N-1 for Image cores={cores})"
        ));
    }
    Ok(Some(n as usize))
}

fn value_as_u64(v: &Value) -> Option<u64> {
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

fn driver_is_virtio_blk(graph: &ImageGraph, decl: &crate::eval::image::DriverDecl) -> bool {
    decl.args
        .iter()
        .find(|a| a.label == "device")
        .and_then(|a| match &a.value {
            Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
            _ => None,
        })
        .and_then(|i| graph.devices.get(i))
        .is_some_and(|d| matches!(&d.device_type, Type::Named(n, _) if n == "VirtioBlock"))
}

fn dma_pool_bytes_by_driver(
    graph: &ImageGraph,
    layout_ctx: &LayoutCtx,
) -> Result<BTreeMap<usize, u64>, String> {
    let mut out: BTreeMap<usize, u64> = BTreeMap::new();
    for (name, pool) in &graph.dma_pools {
        let device =
            pool.args
                .iter()
                .find(|a| a.label == "device")
                .and_then(|a| match &a.value {
                    Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
                    Value::ImageDecl(ImageDeclRef::Driver(di)) => {
                        graph.drivers.get(*di).and_then(|d| {
                            d.args.iter().find(|a| a.label == "device").and_then(|a| {
                                match &a.value {
                                    Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
                                    _ => None,
                                }
                            })
                        })
                    }
                    _ => None,
                });
        let Some(dev_i) = device else {
            continue;
        };
        let Some(driver_i) = graph.drivers.iter().position(|d| {
            d.args.iter().any(|a| {
                matches!(&a.value, Value::ImageDecl(ImageDeclRef::Device(i)) if *i == dev_i)
                    && a.label == "device"
            })
        }) else {
            continue;
        };
        let count = pool
            .args
            .iter()
            .find(|a| a.label == "count" || a.label == "slots")
            .and_then(|a| value_as_u64(&a.value))
            .ok_or_else(|| {
                format!("dma_pool `{name}` has no readable `count=`/`slots=` for placement bytes")
            })?;
        let payload = mwir::size_of(&pool.payload_type, layout_ctx)
            .map_err(|e| format!("dma_pool `{name}` payload size for placement: {e}"))?
            as u64;
        *out.entry(driver_i).or_insert(0) += count.saturating_mul(payload);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::image::{ActorDecl, DeviceDecl, DriverDecl};

    fn tv(ty: Type, value: Value) -> crate::eval::image::TypedValue {
        crate::eval::image::TypedValue { ty, value }
    }

    fn arg(label: &str, ty: Type, value: Value) -> DeclArg {
        DeclArg {
            label: label.to_string(),
            ty,
            value,
        }
    }

    #[test]
    fn core_out_of_range_fails_closed() {
        let mut g = ImageGraph::new(
            tv(Type::Static(Box::new(Type::Str)), Value::Str(b"t".to_vec())),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        g.cores = 2;
        g.actors.push(ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![
                arg("mailbox", Type::U32, Value::U32(4)),
                arg("core", Type::U32, Value::U32(2)),
            ],
        });
        let err = check_annotations(&g).expect_err("core=2");
        assert!(err.contains("core=2"), "{err}");
        assert!(err.contains("0..2"), "{err}");
        assert!(err.contains("cores=2"), "{err}");
    }

    #[test]
    fn core_out_of_range_uses_image_n_not_machine_width() {
        let mut g = ImageGraph::new(
            tv(Type::Static(Box::new(Type::Str)), Value::Str(b"t".to_vec())),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        g.actors.push(ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![
                arg("mailbox", Type::U32, Value::U32(4)),
                arg("core", Type::U32, Value::U32(1)),
            ],
        });
        let err = check_annotations(&g).expect_err("core=1 with N=1");
        assert!(err.contains("0..1"), "{err}");
        assert!(err.contains("cores=1"), "{err}");
    }

    #[test]
    fn virtio_blk_refuses_core_nonzero() {
        let mut g = ImageGraph::new(
            tv(Type::Static(Box::new(Type::Str)), Value::Str(b"t".to_vec())),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        g.cores = 2;
        g.devices.push(DeviceDecl {
            device_type: Type::Named("VirtioBlock".to_string(), vec![]),
            args: vec![],
        });
        g.drivers.push(DriverDecl {
            actor_type: Type::Named("BlkDriver".to_string(), vec![]),
            args: vec![
                arg(
                    "device",
                    Type::Named("ImageDecl".to_string(), vec![]),
                    Value::ImageDecl(ImageDeclRef::Device(0)),
                ),
                arg("core", Type::U32, Value::U32(1)),
            ],
        });
        let err = check_annotations(&g).expect_err("blk on core 1");
        assert!(err.contains("virtio-blk"), "{err}");
        assert!(err.contains("core=1"), "{err}");
    }

    #[test]
    fn pick_core_prefers_lexicographically_smallest_load() {
        let loads = [(1, 100), (0, 50), (0, 50)];
        assert_eq!(pick_core(&loads), 1);
        let loads2 = [(0, 10), (0, 20), (0, 5)];
        assert_eq!(pick_core(&loads2), 2);
    }

    #[test]
    fn sort_order_is_descending_work_bytes_then_identity() {
        let a = Placeable {
            id: ImageDeclRef::Actor(0),
            type_name: "A".into(),
            work: 0,
            bytes_state: 100,
            bytes_mailbox: 0,
            bytes_pool: 0,
            explicit: None,
            pinned_virtio_blk: false,
        };
        let b = Placeable {
            id: ImageDeclRef::Actor(1),
            type_name: "B".into(),
            work: 0,
            bytes_state: 200,
            bytes_mailbox: 0,
            bytes_pool: 0,
            explicit: None,
            pinned_virtio_blk: false,
        };
        let c = Placeable {
            id: ImageDeclRef::Driver(0),
            type_name: "D".into(),
            work: 0,
            bytes_state: 200,
            bytes_mailbox: 0,
            bytes_pool: 0,
            explicit: None,
            pinned_virtio_blk: false,
        };
        let mut v = [&a, &b, &c];
        v.sort_by(|x, y| {
            y.work
                .cmp(&x.work)
                .then_with(|| y.bytes().cmp(&x.bytes()))
                .then_with(|| x.id.cmp(&y.id))
        });
        assert_eq!(v[0].id, ImageDeclRef::Driver(0));
        assert_eq!(v[1].id, ImageDeclRef::Actor(1));
        assert_eq!(v[2].id, ImageDeclRef::Actor(0));
    }
}
