use std::collections::{BTreeSet, VecDeque};

use super::rule::{EmittedWord, MemClass, MemRef};
use super::sweep::SweepPoint;
use super::table::CostTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineId {
    Stack(u64),
    Cold(u8, u64),
}

impl LineId {
    pub fn of(m: MemRef, line_bytes: u64) -> Option<LineId> {
        let line_bytes = line_bytes.max(1);
        match m.class {
            MemClass::Stack => Some(LineId::Stack(m.key / line_bytes)),
            MemClass::Cold => {
                let base = m.base_reg()?;
                let imm = m.key & 0x0000_FFFF_FFFF_FFFF;
                Some(LineId::Cold(base, imm / line_bytes))
            }
        }
    }

    fn index(self) -> u64 {
        match self {
            LineId::Stack(i) => i,
            LineId::Cold(_, i) => i,
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

    fn touch(&mut self, l: LineId) -> bool {
        let s = self.set_of(l);
        let set = &mut self.sets[s];
        match set.iter().position(|&e| e == l) {
            Some(at) => {
                let e = set.remove(at);
                set.insert(0, e);
                true
            }
            None => false,
        }
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
    store_buffer: VecDeque<Option<MemRef>>,
    store_buffer_depth: usize,
    seen: BTreeSet<LineId>,
    lat_l1d_hit: u64,
    lat_l2: u64,
    lat_l3: u64,
    lat_dram: u64,
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
            store_buffer: VecDeque::new(),
            store_buffer_depth: store_buffer_depth(table),
            seen: BTreeSet::new(),
            lat_l1d_hit: geom(table, "lat_l1d_hit"),
            lat_l2: point.get("l2_latency"),
            lat_l3: point.get("l3_latency"),
            lat_dram: point.get("dram_latency"),
            lat_store: table
                .latency_row("store")
                .map(|r| r.lat)
                .unwrap_or_else(|| panic!("cost table: [latency.store] is required")),
            lat_forward: point.get("store_to_load_forwarding"),
        }
    }

    pub fn clear(&mut self) {
        self.l1d.clear();
        self.store_buffer.clear();
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
        if self.store_buffer.contains(&Some(m)) {
            self.fill(line);
            return MemVerdict {
                level: MemLevel::Forwarded,
                latency: self.lat_forward,
            };
        }
        if self.l1d.touch(line) {
            self.l2.touch(line);
            self.l3.touch(line);
            return MemVerdict {
                level: MemLevel::L1dHit,
                latency: self.lat_l1d_hit,
            };
        }
        if self.l2.touch(line) {
            self.l3.touch(line);
            self.l1d.install(line);
            return MemVerdict {
                level: MemLevel::L2,
                latency: self.lat_l2,
            };
        }
        if self.l3.touch(line) {
            self.l1d.install(line);
            self.l2.install(line);
            return MemVerdict {
                level: MemLevel::L3,
                latency: self.lat_l3,
            };
        }
        if self.seen.insert(line) {
            let latency = match m.class {
                MemClass::Stack => self.lat_l2,
                MemClass::Cold => self.lat_l3,
            };
            self.fill(line);
            return MemVerdict {
                level: MemLevel::Compulsory,
                latency,
            };
        }
        self.fill(line);
        MemVerdict {
            level: MemLevel::Dram,
            latency: self.lat_dram,
        }
    }

    fn store(&mut self, mem: Option<MemRef>) -> MemVerdict {
        let latency = self.lat_store;
        if let Some(line) = mem.and_then(|m| LineId::of(m, self.line_bytes)) {
            self.seen.insert(line);
            self.fill(line);
        }
        self.store_buffer
            .push_back(mem.filter(|m| LineId::of(*m, self.line_bytes).is_some()));
        while self.store_buffer.len() > self.store_buffer_depth {
            self.store_buffer.pop_front();
        }
        MemVerdict {
            level: MemLevel::Buffered,
            latency,
        }
    }

    fn unresolved(&self) -> MemVerdict {
        MemVerdict {
            level: MemLevel::Unresolved,
            latency: self.lat_l3,
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

    pub fn store_buffer_depth(&self) -> usize {
        self.store_buffer_depth
    }
}

fn store_buffer_depth(table: &CostTable) -> usize {
    let per_pipe = table
        .pipeline_row("cap_l_each")
        .map(|r| r.value)
        .unwrap_or(1)
        .max(1);
    let pipes = table
        .pipeline_row("port_l")
        .map(|r| r.value)
        .unwrap_or(1)
        .max(1);
    per_pipe.saturating_mul(pipes).max(1) as usize
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
        EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[MEM_SP_REG])
            .with_mem(MemRef::stack(offset))
    }

    fn store(offset: u64) -> EmittedWord {
        EmittedWord::new(0, String::new(), CostRule::Store, None, &[MEM_SP_REG, 0])
            .with_mem(MemRef::stack(offset))
    }

    fn cold(base: u8, imm: u64) -> EmittedWord {
        EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[base])
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
        assert_eq!(LineId::of(MemRef::cold_unique(0), 64), None);
        assert_eq!(LineId::of(MemRef::cold_unique(9), 64), None);
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
    fn reuse_at_distance_one_and_beyond_capacity_select_different_leaves() {
        let t = table();
        let mut s = state(&t);
        assert_eq!(s.access(&load(0)).level, MemLevel::Compulsory);
        let near = s.access(&load(0));
        assert_eq!(near.level, MemLevel::L1dHit);
        assert_eq!(near.latency, 4);

        let mut s = state(&t);
        assert_eq!(s.access(&load(0)).level, MemLevel::Compulsory);
        for line in 1..=1024u64 {
            s.access(&load(line * 64));
        }
        let far = s.access(&load(0));
        assert_eq!(
            far.level,
            MemLevel::L2,
            "1025 distinct lines cannot all be L1D-resident"
        );
        assert_eq!(far.latency, 11);
        assert_ne!(near.level, far.level, "the two distances must differ");
    }

    #[test]
    fn five_way_conflict_inside_capacity_still_misses_on_four_way() {
        let t = table();
        let mut s = state(&t);
        let stride = 256u64 * 64;
        for k in 0..5u64 {
            s.access(&load(k * stride));
        }
        let reuse = s.access(&load(0));
        assert_eq!(
            reuse.level,
            MemLevel::L2,
            "5 lines deep in one 4-way set must evict the first"
        );
        let mut s = state(&t);
        for k in 0..5u64 {
            s.access(&load(k * 64));
        }
        assert_eq!(
            s.access(&load(0)).level,
            MemLevel::L1dHit,
            "the same five-line working set without the conflict must hit"
        );
        assert!(5 * 64 < 65536);
    }

    #[test]
    fn l1_miss_that_hits_l2_takes_the_eleven_cycle_path() {
        let t = table();
        assert_eq!(
            t.geometry("l2_inclusive_of_l1d").expect("row").value,
            1,
            "the profile must declare strict inclusivity"
        );
        let mut s = state(&t);
        let stride = 256u64 * 64;
        for k in 0..5u64 {
            s.access(&load(k * stride));
        }
        let v = s.access(&load(0));
        assert_eq!(v.level, MemLevel::L2);
        assert_eq!(v.latency, 11);
        assert!(s.l1d_resident(LineId::Stack(0)));
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
    fn eviction_from_every_level_reaches_the_dram_leaf() {
        let t = table();
        let mut s = state(&t);
        let (_, _, l3_sets) = s.set_counts();
        let stride = l3_sets as u64 * 64;
        for k in 0..17u64 {
            s.access(&load(k * stride));
        }
        let v = s.access(&load(0));
        assert_eq!(v.level, MemLevel::Dram);
        assert_eq!(v.latency, 347);
    }

    #[test]
    fn compulsory_reference_is_charged_at_its_class_home_level() {
        let t = table();
        let mut s = state(&t);
        let stack = s.access(&load(0));
        assert_eq!(stack.level, MemLevel::Compulsory);
        assert_eq!(stack.latency, 11);
        let mut s = state(&t);
        let first_cold = s.access(&cold(28, 0));
        assert_eq!(first_cold.level, MemLevel::Compulsory);
        assert_eq!(first_cold.latency, 35);
        assert_eq!(s.access(&cold(28, 8)).level, MemLevel::L1dHit);
    }

    #[test]
    fn cold_unique_never_reuses_anything() {
        let t = table();
        let mut s = state(&t);
        let uniq = |seq: u64| {
            EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[0])
                .with_mem(MemRef::cold_unique(seq))
        };
        for seq in 0..4u64 {
            let v = s.access(&uniq(seq));
            assert_eq!(v.level, MemLevel::Unresolved);
            assert_eq!(v.latency, 35);
        }
        assert_eq!(s.access(&uniq(0)).level, MemLevel::Unresolved);
        let untagged = EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[0]);
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
    fn an_unresolved_store_occupies_a_buffer_entry() {
        let t = table();
        let lo = SweepPoint::pinned(&t).with("store_to_load_forwarding", 1);
        let mut s = MemState::new(&t, &lo);
        let depth = s.store_buffer_depth();
        s.access(&store(8));
        for seq in 0..depth as u64 {
            let w = EmittedWord::new(0, String::new(), CostRule::Store, None, &[0])
                .with_mem(MemRef::cold_unique(seq));
            assert_eq!(s.access(&w).level, MemLevel::Buffered);
        }
        assert_eq!(
            s.access(&load(8)).level,
            MemLevel::L1dHit,
            "the unresolved stores pushed slot 8 out of the buffer"
        );
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
    fn store_buffer_is_bounded_and_draining_returns_the_load_to_cache() {
        let t = table();
        let s = state(&t);
        let depth = s.store_buffer_depth();
        assert_eq!(depth, 4, "cap_l_each (2) x the pipes port_l names (2)");
        let mut s = state(&t);
        s.access(&store(0));
        for k in 1..=depth as u64 {
            s.access(&store(k * 64));
        }
        let v = s.access(&load(0));
        assert_eq!(
            v.level,
            MemLevel::L1dHit,
            "a drained store leaves its line in L1D (write-back, write-allocate)"
        );
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
    fn clear_drops_l1d_and_the_buffer_but_not_l2() {
        let t = table();
        let mut s = state(&t);
        s.access(&load(8));
        s.access(&store(8));
        s.clear();
        assert!(!s.l1d_resident(LineId::Stack(0)), "L1D must be gone");
        assert!(s.l2_resident(LineId::Stack(0)), "L2 must survive a call");
        let v = s.access(&load(8));
        assert_eq!(v.level, MemLevel::L2);
        assert_eq!(v.latency, 11);
        assert!(s.l1d_resident(LineId::Stack(0)));
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
                distinct_lines(n) > one_line(n),
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
        assert_eq!(d6 - d5, 11, "a compulsory frame line is lat_l2");
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
    fn every_bracketed_leaf_comes_from_the_point() {
        let t = table();
        let pinned = SweepPoint::pinned(&t);
        for (dim, lo) in [
            ("l2_latency", 9u64),
            ("l3_latency", 26),
            ("dram_latency", 289),
            ("store_to_load_forwarding", 1),
        ] {
            let moved = pinned.with(dim, lo);
            let a = MemState::new(&t, &pinned);
            let b = MemState::new(&t, &moved);
            let read = |s: &MemState| match dim {
                "l2_latency" => s.lat_l2,
                "l3_latency" => s.lat_l3,
                "dram_latency" => s.lat_dram,
                _ => s.lat_forward,
            };
            assert_ne!(read(&a), read(&b), "`{dim}` does not reach the model");
            assert_eq!(read(&b), lo);
        }
        assert_eq!(MemState::new(&t, &pinned).lat_l1d_hit, 4);
    }
}
