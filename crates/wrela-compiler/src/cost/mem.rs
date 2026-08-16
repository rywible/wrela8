use std::collections::BTreeSet;

use super::rule::{EmittedWord, MemRef};
use super::sweep::SweepPoint;
use super::table::CostTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineId {
    Stack(u64),
    Flow(u64, u64),
    Static(u64, u64),
    Mmio(u64, u64),
    /// Retained for callers constructing old synthetic references directly.
    Cold(u8, u64),
}

impl LineId {
    pub fn of(m: MemRef, line_bytes: u64) -> Option<LineId> {
        let line_bytes = line_bytes.max(1);
        match m.target {
            super::rule::MemTarget::Stack { function, offset } => {
                if function == 0 {
                    Some(LineId::Stack(offset / line_bytes))
                } else {
                    Some(LineId::Flow(function, offset / line_bytes))
                }
            }
            super::rule::MemTarget::FlowFrame { function, offset } => {
                Some(LineId::Flow(function, offset / line_bytes))
            }
            super::rule::MemTarget::Static { symbol, offset } => {
                if symbol <= u8::MAX as u64 {
                    Some(LineId::Cold(symbol as u8, offset / line_bytes))
                } else {
                    Some(LineId::Static(symbol, offset / line_bytes))
                }
            }
            super::rule::MemTarget::Mmio { device, offset } => {
                Some(LineId::Mmio(device, offset / line_bytes))
            }
            super::rule::MemTarget::Unknown { .. } => None,
        }
    }

    fn index(self) -> u64 {
        match self {
            LineId::Stack(i) | LineId::Cold(_, i) => i,
            LineId::Flow(_, i) | LineId::Static(_, i) | LineId::Mmio(_, i) => i,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemLevel {
    Forwarded,
    L1dHit,
    L2,
    L3,
    Dram,
    Compulsory,
    Buffered,
    Unresolved,
}

impl MemLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forwarded => "forwarded",
            Self::L1dHit => "l1_hit",
            Self::L2 => "l2",
            Self::L3 => "l3",
            Self::Dram => "dram",
            Self::Compulsory => "compulsory",
            Self::Buffered => "buffered",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemVerdict {
    pub level: MemLevel,
    pub latency: u64,
}

#[derive(Debug, Clone)]
struct Level {
    ways: usize,
    sets: Vec<Vec<LineId>>,
}

impl Level {
    fn new(bytes: u64, line_bytes: u64, ways: u64) -> Level {
        let line_bytes = line_bytes.max(1);
        let ways = ways.max(1);
        let set_count = (bytes / line_bytes / ways).max(1) as usize;
        Level {
            ways: ways as usize,
            sets: vec![Vec::new(); set_count],
        }
    }

    fn set_of(&self, l: LineId) -> usize {
        (l.index() % self.sets.len() as u64) as usize
    }

    fn install(&mut self, l: LineId) {
        let s = self.set_of(l);
        let ways = self.ways;
        let set = &mut self.sets[s];
        if let Some(at) = set.iter().position(|&e| e == l) {
            let e = set.remove(at);
            set.insert(0, e);
            return;
        }
        set.insert(0, l);
        while set.len() > ways {
            set.pop();
        }
    }

    fn resident(&self, l: LineId) -> bool {
        self.sets[self.set_of(l)].contains(&l)
    }

    fn clear(&mut self) {
        for s in &mut self.sets {
            s.clear();
        }
    }

    fn set_count(&self) -> usize {
        self.sets.len()
    }
}

#[derive(Debug, Clone)]
pub struct MemState {
    line_bytes: u64,
    l1d: Level,
    l2: Level,
    l3: Level,
    pending_stores: BTreeSet<MemRef>,
    seen: BTreeSet<LineId>,
    lat_l1d_hit: u64,
    lat_store: u64,
    lat_forward: u64,
}

fn geom(table: &CostTable, key: &str) -> u64 {
    table
        .geometry(key)
        .unwrap_or_else(|| panic!("cost table: [geometry.{key}] is required by the memory model"))
        .value
}

impl MemState {
    pub fn new(table: &CostTable, point: &SweepPoint) -> MemState {
        let line_bytes = geom(table, "l1d_line_bytes").max(1);
        let l3_bytes = point.get("effective_l3_bytes");
        MemState {
            line_bytes,
            l1d: Level::new(
                geom(table, "l1d_bytes"),
                line_bytes,
                geom(table, "l1d_ways"),
            ),
            l2: Level::new(geom(table, "l2_bytes"), line_bytes, geom(table, "l2_ways")),
            l3: Level::new(l3_bytes, line_bytes, geom(table, "l3_ways")),
            pending_stores: BTreeSet::new(),
            seen: BTreeSet::new(),
            lat_l1d_hit: geom(table, "lat_l1d_hit"),
            lat_store: table
                .latency_row("store")
                .map(|r| r.lat)
                .unwrap_or_else(|| panic!("cost table: [latency.store] is required")),
            lat_forward: point.get("store_to_load_forwarding"),
        }
    }

    pub fn clear(&mut self) {
        self.l1d.clear();
        self.pending_stores.clear();
    }

    /// A call is a control/alias boundary, not a cache flush.
    pub fn call_boundary(&mut self) {
        self.pending_stores.clear();
    }

    /// Barriers end forwarding knowledge but preserve cache residency.
    pub fn barrier(&mut self) {
        self.pending_stores.clear();
    }

    pub fn access(&mut self, ew: &EmittedWord) -> MemVerdict {
        if ew.rule.is_load() {
            self.load(ew.mem)
        } else {
            self.store(ew.mem)
        }
    }

    fn load(&mut self, mem: Option<MemRef>) -> MemVerdict {
        let Some(m) = mem else {
            return self.unresolved();
        };
        let Some(line) = LineId::of(m, self.line_bytes) else {
            return self.unresolved();
        };
        if self.pending_stores.contains(&m) {
            self.fill(line);
            return MemVerdict {
                level: MemLevel::Forwarded,
                latency: self.lat_forward,
            };
        }
        // A static block-frequency vector is not an execution trace.  Rank
        // every resolved non-forwarded target at the documented L1 floor and
        // price cache/TLB capacity from final-address footprints instead.
        // The hierarchy below is retained only as diagnostic state; it never
        // makes deleting one instruction look like deleting a compulsory miss.
        self.seen.insert(line);
        self.fill(line);
        MemVerdict {
            level: MemLevel::L1dHit,
            latency: self.lat_l1d_hit,
        }
    }

    fn store(&mut self, mem: Option<MemRef>) -> MemVerdict {
        let latency = self.lat_store;
        let Some(mem) = mem else {
            self.pending_stores.clear();
            return MemVerdict {
                level: MemLevel::Buffered,
                latency,
            };
        };
        let Some(line) = LineId::of(mem, self.line_bytes) else {
            // An unknown store may alias every exact pending target.
            self.pending_stores.clear();
            return MemVerdict {
                level: MemLevel::Buffered,
                latency,
            };
        };
        self.seen.insert(line);
        self.fill(line);
        // There is no invented queue depth.  Distinct compiler-proven targets
        // coexist until a call, barrier, or possibly aliasing unknown store.
        self.pending_stores.insert(mem);
        MemVerdict {
            level: MemLevel::Buffered,
            latency,
        }
    }

    fn unresolved(&self) -> MemVerdict {
        MemVerdict {
            level: MemLevel::Unresolved,
            // Unknown addresses get the documented L1 floor in the rank
            // column; capacity/placement stress is priced elsewhere.
            latency: self.lat_l1d_hit,
        }
    }

    fn fill(&mut self, line: LineId) {
        self.l1d.install(line);
        self.l2.install(line);
        self.l3.install(line);
    }

    pub fn l1d_resident(&self, line: LineId) -> bool {
        self.l1d.resident(line)
    }

    pub fn l2_resident(&self, line: LineId) -> bool {
        self.l2.resident(line)
    }

    pub fn set_counts(&self) -> (usize, usize, usize) {
        (
            self.l1d.set_count(),
            self.l2.set_count(),
            self.l3.set_count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::rule::{CostRule, MEM_SP_REG};
    use crate::cost::table::load_default;

    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn state(table: &CostTable) -> MemState {
        MemState::new(table, &SweepPoint::pinned(table))
    }

    fn load(offset: u64) -> EmittedWord {
        EmittedWord::gpr(0, String::new(), CostRule::Load, Some(1), &[MEM_SP_REG])
            .with_mem(MemRef::stack(offset))
    }

    fn store(offset: u64) -> EmittedWord {
        EmittedWord::gpr(0, String::new(), CostRule::Store, None, &[MEM_SP_REG, 0])
            .with_mem(MemRef::stack(offset))
    }

    fn cold(base: u8, imm: u64) -> EmittedWord {
        EmittedWord::gpr(0, String::new(), CostRule::Load, Some(1), &[base])
            .with_mem(MemRef::cold_stable(base, imm))
    }

    #[test]
    fn line_identity_per_memref_class() {
        assert_eq!(LineId::of(MemRef::stack(0), 64), Some(LineId::Stack(0)));
        assert_eq!(LineId::of(MemRef::stack(63), 64), Some(LineId::Stack(0)));
        assert_eq!(LineId::of(MemRef::stack(64), 64), Some(LineId::Stack(1)));
        assert_eq!(
            LineId::of(MemRef::cold_stable(28, 0), 64),
            Some(LineId::Cold(28, 0))
        );
        assert_eq!(
            LineId::of(MemRef::cold_stable(28, 127), 64),
            Some(LineId::Cold(28, 1))
        );
        assert_ne!(
            LineId::of(MemRef::cold_stable(28, 0), 64),
            LineId::of(MemRef::cold_stable(27, 0), 64),
            "two bases must not share a line identity"
        );
        let packed = MemRef::cold_stable(28, 0).key;
        assert_eq!(
            packed / 64,
            (28u64 << 48) / 64,
            "packed/64 folds in the base"
        );
        assert_ne!(packed / 64, 0);
        assert_eq!(
            LineId::of(MemRef::cold_unique(0), 64),
            Some(LineId::Static(u64::MAX, 0))
        );
        assert_eq!(
            LineId::of(MemRef::cold_unique(9), 64),
            Some(LineId::Static(u64::MAX, 9))
        );
    }

    #[test]
    fn set_counts_come_from_the_profile_geometry() {
        let t = table();
        let s = state(&t);
        assert_eq!(s.set_counts(), (256, 1024, 1024));
        let point = SweepPoint::pinned(&t).with("effective_l3_bytes", 2 * 1024 * 1024);
        let big = MemState::new(&t, &point);
        assert_eq!(big.set_counts(), (256, 1024, 2048));
    }

    #[test]
    fn rank_is_removal_safe_across_reuse_distances() {
        let t = table();
        let mut near = state(&t);
        assert_eq!(near.access(&load(0)).level, MemLevel::L1dHit);
        assert_eq!(near.access(&load(0)).latency, 4);

        let mut far = state(&t);
        far.access(&load(0));
        for line in 1..=1024u64 {
            far.access(&load(line * 64));
        }
        let reuse = far.access(&load(0));
        assert_eq!(reuse.level, MemLevel::L1dHit);
        assert_eq!(reuse.latency, 4);
    }

    #[test]
    fn set_conflicts_are_priced_by_footprint_not_block_rank() {
        let t = table();
        let mut s = state(&t);
        let stride = 256u64 * 64;
        for k in 0..5u64 {
            s.access(&load(k * stride));
        }
        assert_eq!(s.access(&load(0)).level, MemLevel::L1dHit);
    }

    #[test]
    fn resolved_loads_take_the_l1_rank_floor() {
        let t = table();
        let mut s = state(&t);
        let stride = 256u64 * 64;
        for k in 0..5u64 {
            s.access(&load(k * stride));
        }
        let v = s.access(&load(0));
        assert_eq!(v.level, MemLevel::L1dHit);
        assert_eq!(v.latency, 4);
    }

    #[test]
    fn l2_eviction_can_never_strand_a_line_in_l1d() {
        let t = table();
        let mut s = state(&t);
        let (l1_sets, l2_sets, _) = s.set_counts();
        assert_eq!(l2_sets % l1_sets, 0, "L2 sets must partition over L1D sets");
        let stride = l2_sets as u64 * 64;
        for k in 0..9u64 {
            s.access(&load(k * stride));
        }
        assert!(!s.l2_resident(LineId::Stack(0)));
        assert!(
            !s.l1d_resident(LineId::Stack(0)),
            "a line evicted from L2 must already be gone from L1D"
        );
    }

    #[test]
    fn hierarchy_eviction_does_not_invent_a_dram_rank_charge() {
        let t = table();
        let mut s = state(&t);
        let (_, _, l3_sets) = s.set_counts();
        let stride = l3_sets as u64 * 64;
        for k in 0..17u64 {
            s.access(&load(k * stride));
        }
        let v = s.access(&load(0));
        assert_eq!(v.level, MemLevel::L1dHit);
        assert_eq!(v.latency, 4);
    }

    #[test]
    fn every_resolved_reference_uses_the_l1_rank_floor() {
        let t = table();
        let mut s = state(&t);
        let stack = s.access(&load(0));
        assert_eq!(stack.level, MemLevel::L1dHit);
        assert_eq!(stack.latency, 4);
        let mut s = state(&t);
        let first_cold = s.access(&cold(28, 0));
        assert_eq!(first_cold.level, MemLevel::L1dHit);
        assert_eq!(first_cold.latency, 4);
        assert_eq!(s.access(&cold(28, 8)).level, MemLevel::L1dHit);
    }

    #[test]
    fn synthetic_cold_lines_have_stable_unique_provenance() {
        let t = table();
        let mut s = state(&t);
        let uniq = |seq: u64| {
            EmittedWord::gpr(0, String::new(), CostRule::Load, Some(1), &[0])
                .with_mem(MemRef::cold_unique(seq))
        };
        for seq in 0..4u64 {
            let v = s.access(&uniq(seq));
            assert_eq!(v.level, MemLevel::L1dHit);
            assert_eq!(v.latency, 4);
        }
        assert_eq!(s.access(&uniq(0)).level, MemLevel::L1dHit);
        let untagged = EmittedWord::gpr(0, String::new(), CostRule::Load, Some(1), &[0]);
        assert_eq!(s.access(&untagged).level, MemLevel::Unresolved);
    }

    #[test]
    fn forwarding_matches_the_slot_and_not_its_line() {
        let t = table();
        let lo = SweepPoint::pinned(&t).with("store_to_load_forwarding", 1);
        let mut s = MemState::new(&t, &lo);
        assert_eq!(
            LineId::of(MemRef::stack(8), 64),
            LineId::of(MemRef::stack(16), 64)
        );
        s.access(&store(8));
        let other = s.access(&load(16));
        assert_eq!(
            other.level,
            MemLevel::L1dHit,
            "a load of a different slot of the stored line must not forward"
        );
        assert_eq!(other.latency, 4, "it pays the L1D hit the store allocated");
        let same = s.access(&load(8));
        assert_eq!(same.level, MemLevel::Forwarded);
        assert_eq!(same.latency, 1);
    }

    #[test]
    fn an_unknown_store_ends_exact_forwarding_knowledge() {
        let t = table();
        let lo = SweepPoint::pinned(&t).with("store_to_load_forwarding", 1);
        let mut s = MemState::new(&t, &lo);
        s.access(&store(8));
        let unknown = EmittedWord::gpr(0, String::new(), CostRule::Store, None, &[0])
            .with_mem(MemRef::unknown(7, Some(0), 0));
        assert_eq!(s.access(&unknown).level, MemLevel::Buffered);
        assert_eq!(s.access(&load(8)).level, MemLevel::L1dHit);
    }

    #[test]
    fn store_then_load_forwards_rather_than_taking_the_l1_path() {
        let t = table();
        let mut s = state(&t);
        assert_eq!(s.access(&store(8)).level, MemLevel::Buffered);
        let fwd = s.access(&load(8));
        assert_eq!(fwd.level, MemLevel::Forwarded);
        assert_eq!(fwd.latency, 4, "pinned at lat_l1d_hit");

        let mut s = state(&t);
        s.access(&load(8));
        assert_eq!(s.access(&load(8)).level, MemLevel::L1dHit);

        let lo = SweepPoint::pinned(&t).with("store_to_load_forwarding", 1);
        let mut s = MemState::new(&t, &lo);
        s.access(&store(8));
        let fwd = s.access(&load(8));
        assert_eq!(fwd.level, MemLevel::Forwarded);
        assert_eq!(
            fwd.latency, 1,
            "the forwarding latency must come from the point"
        );
    }

    #[test]
    fn non_aliasing_stores_do_not_evict_exact_forwarding_knowledge() {
        let t = table();
        let mut s = state(&t);
        s.access(&store(0));
        for k in 1..=32u64 {
            s.access(&store(k * 64));
        }
        assert_eq!(s.access(&load(0)).level, MemLevel::Forwarded);
    }

    #[test]
    fn a_store_no_longer_invalidates_its_own_line() {
        let t = table();
        let mut s = state(&t);
        s.access(&load(8));
        s.access(&store(8));
        let after = s.access(&load(8));
        assert!(
            matches!(after.level, MemLevel::Forwarded),
            "got {:?}",
            after.level
        );
        assert!(
            after.latency <= 4,
            "a reload after a store must not be dearer than an L1 hit"
        );
    }

    #[test]
    fn clear_ends_forwarding_but_rank_stays_at_the_l1_floor() {
        let t = table();
        let mut s = state(&t);
        s.access(&store(8));
        assert_eq!(s.access(&load(8)).level, MemLevel::Forwarded);
        s.clear();
        let v = s.access(&load(8));
        assert_eq!(v.level, MemLevel::L1dHit);
        assert_eq!(v.latency, 4);
    }

    #[test]
    fn a_dead_access_never_lowers_a_later_verdict() {
        let t = table();
        let cost = |dead: bool| -> u64 {
            let mut s = state(&t);
            let mut total = 0u64;
            total += s.access(&load(0)).latency;
            if dead {
                total += s.access(&store(1 << 20)).latency;
                total += s.access(&load(1 << 21)).latency;
            }
            total += s.access(&load(0)).latency;
            total += s.access(&load(64)).latency;
            total
        };
        assert!(
            cost(true) >= cost(false),
            "dead accesses lowered the total: {} < {}",
            cost(true),
            cost(false)
        );
        let mut s = state(&t);
        s.access(&load(0));
        s.access(&store(1 << 20));
        s.access(&load(1 << 21));
        assert_eq!(s.access(&load(0)).level, MemLevel::L1dHit);
    }

    #[test]
    fn reuse_distance_subsumes_the_working_set_surcharge() {
        let t = table();
        const DELETED_SURCHARGE: u64 = 2;
        let distinct_lines = |n: u64| -> u64 {
            let mut s = state(&t);
            (0..n).map(|k| s.access(&load(k * 64)).latency).sum()
        };
        let one_line = |n: u64| -> u64 {
            let mut s = state(&t);
            (0..n).map(|k| s.access(&load(k * 8 % 64)).latency).sum()
        };
        for n in [5u64, 6, 8] {
            assert!(
                distinct_lines(n) >= one_line(n),
                "n={n}: {} vs {}",
                distinct_lines(n),
                one_line(n)
            );
        }
        let d5 = distinct_lines(5);
        let d6 = distinct_lines(6);
        assert!(
            d6 - d5 >= DELETED_SURCHARGE,
            "an extra distinct line must cost at least the deleted surcharge: \
             {} vs {DELETED_SURCHARGE}",
            d6 - d5
        );
        assert_eq!(d6 - d5, 4, "a new symbolic frame line is ranked at L1");
        let mut s = state(&t);
        for k in 0..5u64 {
            s.access(&load(k * 64));
        }
        assert_eq!(
            s.access(&load(0)).level,
            MemLevel::L1dHit,
            "5 lines in a 64 KiB 4-way cache conflict with nothing"
        );
    }

    #[test]
    fn forwarding_leaf_comes_from_the_residual_point() {
        let t = table();
        let pinned = SweepPoint::pinned(&t);
        let moved = pinned.with("store_to_load_forwarding", 1);
        let a = MemState::new(&t, &pinned);
        let b = MemState::new(&t, &moved);
        assert_ne!(a.lat_forward, b.lat_forward);
        assert_eq!(b.lat_forward, 1);
        assert_eq!(b.lat_l1d_hit, 4);
    }
}
