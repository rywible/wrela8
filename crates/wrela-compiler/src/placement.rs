//! Actor/driver core placement (plans/M8.md item B, 04-compiler.md §3).
//!
//! Placement is a **build output**: every `@actor` / `@driver` declaration
//! gets exactly one core in `0..VCPUS`. Explicit `core=N` annotations are
//! fixed first; the virtio-blk driver is pinned to core 0; everything else
//! is inferred deterministically from published facts, or rejected by name
//! when a fact the algorithm needs cannot be produced.
//!
//! ## The inference rule (04 §3, narrowed for M8-B — see plans/M8.md)
//!
//! Published sort key for an unfixed placeable: descending
//! `(work, bytes)`, then ascending canonical identity (`driver#i` before
//! `actor#j`, construction order within each kind). Each unfixed placeable
//! is then assigned to the core whose resulting `(work, bytes)` load pair
//! is lexicographically smallest (lowest core index breaks remaining
//! ties).
//!
//! - **work**: proved maximum uninterrupted turn work. The cost model that
//!   would prove it is OUT of M8 (`compiler.costs.predicted-vs-measured`,
//!   M12). Until then every placeable publishes `work=0` with
//!   `work_source=unproved` — equal work, never an invented ranking.
//! - **bytes**: owned image state + mailbox physical bytes + reserved pool
//!   bytes attributed to that placeable (DMA pools via their `device=` to
//!   the bound driver; image pools with no single owner contribute 0).
//!
//! **Single-core floor (M8-B decision):** when no declaration carries an
//! explicit `core=` ≥ 1, the live assignment domain is `{0}` — every
//! unfixed placeable lands on core 0. Secondary vCPUs are not started
//! until item C; spreading unannotated actors across cores would silently
//! strand them. A graph that *does* name `core=` ≥ 1 is a cross-core
//! graph: the full three-core packing runs for the unfixed rest, and the
//! report states the result (dump/report oracles). Execution of off-core
//! placements is item C's multicore boot.
//!
//! Inputs, the chosen core, and the source of each assignment
//! (`explicit` / `pinned-virtio-blk` / `inferred`) are published in the
//! image report so causality is not hidden (04 §7).

use std::collections::BTreeMap;

use crate::eval::image::{DeclArg, ImageDeclRef, ImageGraph};
use crate::eval::value::Value;
use crate::mwir::{self, LayoutCtx};
use crate::sema::types::{self, Type};
use crate::syntax::ast::Module;
use wrela_machine::VCPUS;

/// How a placeable got its core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementSource {
    /// Source wrote `core=N`.
    Explicit,
    /// Virtio-blk driver (and its ISR/bottom half): pinned to core 0.
    PinnedVirtioBlk,
    /// 04 §3 inference (or the M8-B single-core floor).
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

/// One driver or actor's sealed core assignment, with the facts that
/// produced it (04 §3: inputs + inference + final table).
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

/// The whole image's placement table, construction order (drivers then
/// actors, matching the report's declaration walk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementTable {
    pub entries: Vec<PlacementEntry>,
    /// How many cores this image **brings up** — the single-core floor's
    /// own live-domain count (`1`, every M5-M7 image) or `VCPUS` for a
    /// cross-core graph. plans/M8.md item C1 makes this load-bearing: the
    /// image emits one entry block, one `rt_run_one` and one round-robin
    /// cursor per live core, and the report publishes a `CoreEntry` line
    /// for each secondary. One truth: the same predicate that opens the
    /// packing domain below opens the bring-up set.
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

    /// The core every actor instance of struct `type_name` is placed on.
    /// More than one distinct core for one struct name is `None` — the
    /// caller (plans/M8.md item C1's cross-core edge check) fails closed
    /// rather than picking one, because the generated admission routine is
    /// keyed by struct name (`codegen::rt_enqueue_symbol`) and cannot tell
    /// two same-named instances apart.
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

/// Validates every `core=` annotation on drivers/actors without sizing:
/// range `0..VCPUS`, integer shape, and virtio-blk may not leave core 0.
/// Called from `check_sealed` so `--stage=image` rejects bad annotations
/// even when the report is not rendered.
pub fn check_annotations(graph: &ImageGraph) -> Result<(), String> {
    for (i, d) in graph.drivers.iter().enumerate() {
        let name = types::render_type(&d.actor_type);
        let id = format!("driver#{i}");
        match read_core_arg(&d.args, &id)? {
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
        let _ = read_core_arg(&d.args, &id)?;
    }
    Ok(())
}

/// Infers every actor/driver core. `check_annotations` must already have
/// passed (same rules; this re-reads `core=` for the fixed set).
pub fn place(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<PlacementTable, String> {
    check_annotations(graph)?;

    let empty_frames = BTreeMap::new();
    let tables = crate::layout::compute_runtime_tables(graph, modules, layout_ctx, &empty_frames)?
        .unwrap_or_default();

    let pool_by_driver = dma_pool_bytes_by_driver(graph, layout_ctx)?;

    let mut placeables: Vec<Placeable> = Vec::new();
    for (i, d) in graph.drivers.iter().enumerate() {
        let type_name = types::render_type(&d.actor_type);
        let state = tables.drivers.get(i).map(|r| r.state_size).unwrap_or(0);
        // plans/M8.md item D: a `@driver` declared with `mailbox=` owns a
        // real ring plus bookkeeping, sized by the same arithmetic an
        // actor's is below. 04 §3 packs on owned image + mailbox bytes, so
        // reporting 0 here would understate a messageable driver's load and
        // make the Placement line disagree with the Driver line.
        let mailbox = tables
            .drivers
            .get(i)
            .and_then(|r| r.mailbox.as_ref())
            .map(|m| m.capacity * m.slot_size + MAILBOX_BOOKKEEPING)
            .unwrap_or(0);
        let pool = pool_by_driver.get(&i).copied().unwrap_or(0);
        let explicit = read_core_arg(&d.args, &format!("driver#{i}"))?;
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
        let explicit = read_core_arg(&d.args, &format!("actor#{i}"))?;
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

    // Single-core floor: with no explicit core ≥ 1, only core 0 is live
    // (secondary vCPUs start at item C). A cross-core graph opens the
    // full VCPUS-wide packing domain for unfixed placeables.
    let cross_core = placeables
        .iter()
        .any(|p| matches!(p.explicit, Some(c) if c >= 1));
    let live_cores: usize = if cross_core { VCPUS } else { 1 };

    let mut core_load: Vec<(u64, u64)> = vec![(0, 0); live_cores];
    let mut assigned: BTreeMap<ImageDeclRef, (usize, PlacementSource)> = BTreeMap::new();

    // Fixed first: explicit, then virtio-blk pin (explicit already
    // forbids non-zero on virtio-blk via check_annotations).
    for p in &placeables {
        if let Some(core) = p.explicit {
            // `check_annotations` already proved `core < VCPUS`.
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
    // Descending work, then bytes, then ascending identity (ImageDeclRef Ord).
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

/// Appends the placement section (facts only; absent when the image has
/// no actors and no drivers). Fixed order after Supervise in the report.
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

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

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

fn read_core_arg(args: &[DeclArg], id: &str) -> Result<Option<usize>, String> {
    let Some(a) = args.iter().find(|a| a.label == "core") else {
        return Ok(None);
    };
    let Some(n) = value_as_u64(&a.value) else {
        return Err(format!(
            "`{id}` sets `core=...` to a value that is not a plain non-negative integer"
        ));
    };
    if n as usize >= VCPUS {
        return Err(format!(
            "`{id}` sets `core={n}`, which is outside 0..{VCPUS} (06-machine.md §1: the machine \
             has {VCPUS} vCPUs)"
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

/// DMA pool backing bytes attributed to the driver bound to the pool's
/// `device=`. Image pools (`img.pool`) have no single owner here and
/// contribute 0 — inventing a split would hide causality.
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
                        // device= may name the driver in some fixtures; resolve
                        // through that driver's own device= when so.
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
        g.actors.push(ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![
                arg("mailbox", Type::U32, Value::U32(4)),
                arg("core", Type::U32, Value::U32(3)),
            ],
        });
        let err = check_annotations(&g).expect_err("core=3");
        assert!(err.contains("core=3"), "{err}");
        assert!(err.contains("0..3"), "{err}");
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
        // core 1 and 2 both (0,50); lowest index among minima is 1.
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
        // bytes 200: driver#0 before actor#1; then actor#0 at 100.
        assert_eq!(v[0].id, ImageDeclRef::Driver(0));
        assert_eq!(v[1].id, ImageDeclRef::Actor(1));
        assert_eq!(v[2].id, ImageDeclRef::Actor(0));
    }
}
