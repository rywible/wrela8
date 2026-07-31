//! The memory hierarchy: reuse distance, associativity, and the store
//! buffer (plans/M20.md item F, decisions 1603 / 1604 / 1609,
//! `compiler.costs.mem-hierarchy`).
//!
//! **A reuse-distance model with real geometry, not a cache simulator**
//! (decision 1606 / freeze 1622). There is no line-state machine, no
//! coherency protocol, no prefetcher, and no write-back traffic model. What
//! there is: three set-associative LRU structures whose set counts and way
//! counts come out of `[geometry]`, a store buffer, and a hit/miss verdict.
//!
//! ## Line identity
//!
//! A `MemRef` is a class plus a key ([`super::rule`]), and the key's shape
//! differs per class, so the line index is derived per class rather than by
//! dividing the key:
//!
//! - **`Stack`** — the key *is* a frame byte offset from SP, so the line is
//!   `offset / 64`, tagged [`LineId::Stack`].
//! - **`Cold` stable** — the key packs `base_reg << 48 | imm`. The line
//!   comes from the **`imm` part**, keyed by the base register:
//!   `LineId::Cold(base, imm / 64)`. Dividing the *packed* key by 64 would
//!   fold the base register into the line index (`base << 42`), aliasing
//!   unrelated addresses onto one line and silently manufacturing hits.
//!   That is why the base stays a separate tag component instead of being
//!   arithmetic.
//! - **`Cold` unique** — the high bit is set precisely because the address
//!   is *not* a proven `base + imm`. It resolves to **no line at all**
//!   ([`LineId::of`] returns `None`), so it can never reuse anything and
//!   never grants a reuse to anything else.
//!
//! ## Set indexing across bases
//!
//! A set index is `line_index % sets`. For `Stack` the frame *base* is not
//! known, and for `Cold` neither is the base register's value, so only
//! *relative* set alignment inside one base is known. Lines from different
//! bases are indexed into the **same** set space as if their bases were
//! congruent: that manufactures conflicts rather than hiding them, which is
//! decision 1609's direction.
//!
//! ## Initial residency, and why a first touch is not a DRAM access
//!
//! Each scored block starts with empty structures, so the model needs a
//! policy for a **compulsory** reference — the block's first touch of a
//! line. It is charged at its class's *home* level: `Stack` → `lat_l2`,
//! `Cold` → `lat_l3`. Charging DRAM instead would be the arithmetically
//! larger number but **not** the safe one: it is a per-load charge whose
//! removal is a win, so a uniform 347 over-credits *deleting a load* —
//! the same removal-sensitivity `[sweep.dmb_cost]` records for barriers.
//! It is also wrong for this board: the whole wrela image's data working
//! set is orders of magnitude below the 1 MiB effective L3 the housekeeping
//! core leaves (`[sweep.effective_l3_bytes]`), so "unknown" really is
//! bounded by L3 here. The DRAM leaf is therefore reached only by a line
//! that was resident and has been evicted from all three levels.
//!
//! ## L2 is strictly inclusive of L1D
//!
//! `[geometry] l2_inclusive_of_l1d = 1`, so an L1 miss that hits L2 is the
//! `lat_l2` path and **there is no victim path to model** — no victim
//! buffer, no write-back, no fill-from-victim. The inclusion invariant also
//! needs no back-invalidation here, and that is a property of the geometry
//! rather than an omission: L2 has 1024 sets of 8 ways and L1D has 256 sets
//! of 4, so every line in one L2 set maps into a single L1D set. Evicting a
//! line from an L2 set requires 8 more-recently-used lines in that set, all
//! of which land in that one 4-way L1D set — so the line left L1D first.
//! `unit:l2_eviction_can_never_strand_a_line_in_l1d` holds that.
//!
//! ## Known cost directions (decision 1609 makes the direction data)
//!
//! - **Over-cost.** Cross-base set congruence (above). `clear()` drops all
//!   of L1D on a call or SP epoch change, not just the callee's real
//!   footprint.
//! - **Over-cost.** A store does not discharge a later load's *compulsory*
//!   charge only when the buffer has drained and the line was evicted.
//! - **Under-cost.** A `Cold` unique reference resolves to no line, so it
//!   consumes no capacity and evicts nothing. Modelling its pressure would
//!   require inventing its set index.
//! - **Under-cost, recorded rather than fixed.** At the pinned point
//!   `store_to_load_forwarding` (4) equals `lat_l1d_hit` (4), so forwarding
//!   is never cheaper than the cache path it replaces and the pinned model
//!   is monotone. At the bracket's **low** end (1) a store followed by a
//!   load of the same *slot* is cheaper than the load alone — inserting a
//!   store can lower a total. That is the machine's real behaviour, not a
//!   modelling slip, and `[sweep.store_to_load_forwarding]`'s upper bound
//!   is pinned at `lat_l1d_hit` for exactly this reason. The **slot**
//!   qualifier is load-bearing: forwarding is matched per `MemRef`, so the
//!   store-A / load-B pair that shares a line takes the L1D path and the
//!   discount does not spread across the seven other slots of the line.

use std::collections::{BTreeSet, VecDeque};

use super::rule::{EmittedWord, MemClass, MemRef};
use super::sweep::SweepPoint;
use super::table::CostTable;

/// A 64 B line, identified per `MemRef` class. See the module docs for why
/// the `Cold` base register is a tag component and not arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineId {
    /// Frame line: `offset / line_bytes`.
    Stack(u64),
    /// `(base register, imm / line_bytes)`.
    Cold(u8, u64),
}

impl LineId {
    /// The line a `MemRef` names, or `None` when it names none — a `Cold`
    /// **unique** key, which exists precisely because the address is not a
    /// proven `base + imm`.
    pub fn of(m: MemRef, line_bytes: u64) -> Option<LineId> {
        let line_bytes = line_bytes.max(1);
        match m.class {
            MemClass::Stack => Some(LineId::Stack(m.key / line_bytes)),
            MemClass::Cold => {
                // `base_reg()` is `None` exactly for a unique key (high bit
                // set), and the packed layout is `base << 48 | imm`.
                let base = m.base_reg()?;
                let imm = m.key & 0x0000_FFFF_FFFF_FFFF;
                Some(LineId::Cold(base, imm / line_bytes))
            }
        }
    }

    /// The line index the set function is applied to. Deliberately *not*
    /// mixed with the base register: two bases whose lines share an index
    /// share a set, which is the conflict the module docs justify.
    fn index(self) -> u64 {
        match self {
            LineId::Stack(i) => i,
            LineId::Cold(_, i) => i,
        }
    }
}

/// Which structure satisfied an access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemLevel {
    /// Store-to-load forwarding out of the store buffer (inventory row 13).
    Forwarded,
    L1dHit,
    /// L1 miss, L2 hit. L2 is strictly inclusive of L1D, so this is the
    /// whole path — no victim path exists to model.
    L2,
    L3,
    Dram,
    /// This block's first touch of the line, charged at its class's home
    /// level (see the module docs).
    Compulsory,
    /// A store: latency 1 to the **buffer**, not to cache (SOG §3.10).
    Buffered,
    /// An address that resolves to no line (`Cold` unique, or untagged).
    Unresolved,
}

/// One access's verdict: which structure served it, and for how many
/// cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemVerdict {
    pub level: MemLevel,
    pub latency: u64,
}

/// One set-associative level with LRU recency per set.
///
/// **Pseudo-LRU is modelled as LRU** — A76's L1I/L1D replacement is
/// pseudo-LRU (Core TRM) and this is a true LRU. The approximation is named
/// in the `--stage=cost` Assumptions line (`pseudo_lru=modelled_as_lru`),
/// which is where plans/M20.md item F requires it to be visible.
#[derive(Debug, Clone)]
struct Level {
    ways: usize,
    /// Per set, MRU-first. Length is the set count.
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

    /// Move `l` to MRU if resident; report whether it was.
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

    /// Install `l` as MRU, evicting that set's LRU line when the set is
    /// already `ways` deep. This is where a **conflict** verdict comes
    /// from: a set can overflow while total capacity is untouched.
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

/// Carried state of the memory model across one block's word stream.
///
/// Constructed per scored block (`score_words`), which is the grain 04 §5's
/// `Σ f(b)×s(b)` is defined at: a block's first touch of a line is a
/// compulsory reference, and reuse is reuse *within* the block.
#[derive(Debug, Clone)]
pub struct MemState {
    line_bytes: u64,
    l1d: Level,
    l2: Level,
    l3: Level,
    /// SOG §3.10: stores are split into address + data uops and "buffered
    /// and committed in the background", so a load of a **slot** a recent
    /// store wrote is satisfied by **forwarding**. FIFO, oldest at the
    /// front.
    ///
    /// Entries are `MemRef`s, not `LineId`s: forwarding is a store-buffer
    /// address *match*, and A76 matches the accessed bytes, not the line
    /// they sit in. Eight 8-byte spill slots share one 64 B line, so a
    /// line-grained buffer would forward store-slot-A / load-slot-B — an
    /// under-cost of the entire difference between `store_to_load_forwarding`
    /// and `lat_l1d_hit` on every such pair. A non-forwarding load of a
    /// stored line still takes the L1D-hit path, because `store` fills the
    /// line (write-allocate).
    ///
    /// `None` is a store whose address the model could not resolve to a
    /// slot (untagged, or a `Cold` unique key). It matches no load, but it
    /// **occupies** an entry, because a real store buffer is a queue of
    /// stores rather than a queue of known addresses.
    store_buffer: VecDeque<Option<MemRef>>,
    store_buffer_depth: usize,
    /// Lines this block has already referenced. A line absent from all
    /// three levels **and** from this set is a compulsory reference; one
    /// present here is a genuine eviction and takes the DRAM leaf.
    seen: BTreeSet<LineId>,
    lat_l1d_hit: u64,
    lat_l2: u64,
    lat_l3: u64,
    lat_dram: u64,
    lat_store: u64,
    lat_forward: u64,
}

/// One required `[geometry]` row. The parser requires every row in
/// `GEOMETRY_ROWS`, so absence here means the table was built by something
/// other than `cost::table::parse` — fail loudly rather than substitute.
fn geom(table: &CostTable, key: &str) -> u64 {
    table
        .geometry(key)
        .unwrap_or_else(|| panic!("cost table: [geometry.{key}] is required by the memory model"))
        .value
}

impl MemState {
    /// Build the hierarchy for one block at one point of the residual box.
    ///
    /// `lat_l1d_hit` is T1 and unbracketed, so it is read **pinned** out of
    /// `[geometry]`. `lat_l2` / `lat_l3` / `lat_dram` are bracketed, and
    /// the effective L3 **size** is swept too (inventory row 38: the
    /// housekeeping Linux core shares BCM2712's 2 MiB), so all four come
    /// through `point`. The store-to-load forwarding latency is T5 and
    /// **nothing about it is pinned** — it is only ever `point`'s value.
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

    /// Call clobber and SP writes (a new frame epoch) end L1 reuse: a
    /// callee's body is unmodelled text with unmodelled accesses, and stack
    /// offsets after an SP adjust name different bytes.
    ///
    /// L1D and the store buffer go; **L2 and L3 stay**. A callee cannot
    /// plausibly wipe 512 KiB, and clearing L2/L3 here would make the L2
    /// and L3 leaves indistinguishable for every fn with a call in it —
    /// over-cost to the point of destroying the ranking information the
    /// term exists to carry. `seen` also stays: it records what this block
    /// has referenced, and re-charging a compulsory miss on top of the L2
    /// path would double-count the same event.
    pub fn clear(&mut self) {
        self.l1d.clear();
        self.store_buffer.clear();
    }

    /// Whole memory latency of one emitted word, advancing the hierarchy in
    /// **program** order. Loads and stores only — the caller routes by
    /// `CostRule::is_load` / `is_store`, and the ordered accesses (`LDAR` /
    /// `STLR`) take the same path as their plain twins.
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
        // Store-to-load forwarding comes first: the buffer holds data that
        // has not reached cache yet, so it is the only structure that can
        // answer before L1D (SOG §3.10). The match is on the **slot**, not
        // on its line — see the `store_buffer` field docs.
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
            // L2 is strictly inclusive of L1D, so this is the whole path:
            // the line is refilled into L1D and there is no victim to
            // write back or fill from.
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
            // Compulsory: this block's first touch. Charged at the class's
            // home level, for the reasons in the module docs.
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
        // Seen, and gone from all three levels: a real eviction.
        self.fill(line);
        MemVerdict {
            level: MemLevel::Dram,
            latency: self.lat_dram,
        }
    }

    fn store(&mut self, mem: Option<MemRef>) -> MemVerdict {
        // SOG §3.10: latency 1 is to the store buffer, not to cache.
        let latency = self.lat_store;
        if let Some(line) = mem.and_then(|m| LineId::of(m, self.line_bytes)) {
            // A76's L1D is write-back, so a committed store leaves its line
            // resident: the model installs it. Not installing would model a
            // write-through no-allocate cache A76 is not, and would charge
            // every post-store load a fresh miss. Recorded here rather than
            // resolved silently (decision 1609): this is the *documented*
            // behaviour rather than a rounded guess, so accuracy wins over
            // the arithmetically larger number.
            self.seen.insert(line);
            self.fill(line);
        }
        // Every store occupies an entry, including one whose address does
        // not resolve: occupancy is what bounds forwarding, and an
        // unresolved store still drains ahead of the stores behind it.
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

    /// An address the model cannot resolve to a line. Charged at the L3
    /// leaf and **not** installed: there is no line identity to install.
    fn unresolved(&self) -> MemVerdict {
        MemVerdict {
            level: MemLevel::Unresolved,
            latency: self.lat_l3,
        }
    }

    /// Make `line` MRU-resident at every level. `Level::install` already
    /// promotes an already-resident line, so fill and touch are one
    /// operation: the hierarchy is inclusive top to bottom by construction.
    fn fill(&mut self, line: LineId) {
        self.l1d.install(line);
        self.l2.install(line);
        self.l3.install(line);
    }

    /// Test/oracle access: is `line` resident in L1D?
    pub fn l1d_resident(&self, line: LineId) -> bool {
        self.l1d.resident(line)
    }

    /// Test/oracle access: is `line` resident in L2?
    pub fn l2_resident(&self, line: LineId) -> bool {
        self.l2.resident(line)
    }

    /// Set counts, in L1D / L2 / L3 order — the numbers the associativity
    /// term is computed over (256 / 1024 / 1024 at the pinned point).
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

/// Store-buffer depth, in entries.
///
/// **No source gives it.** The SOG has no store-buffer entry anywhere, so
/// the depth is not a fact and cannot be pinned as one. The over-cost
/// direction is the *shallow* end: `store_to_load_forwarding` is bracketed
/// at or below `lat_l1d_hit`, so a forward is never dearer than the cache
/// path it replaces, and forwarding **less** costs more. The shallowest
/// depth at which the mechanism exists at all is one cycle's worth of store
/// address uops, which the profile does give: `cap_l_each` uops per L pipe
/// times the pipes `port_l` names (2 × 2 = 4). Derived rather than invented,
/// and floor-not-fact.
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

    /// Units score against the **committed** `bench/a76-pi5.toml`, so a
    /// profile edit that breaks the memory model shows up here rather than
    /// only in a golden.
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

    // --- line identity ------------------------------------------------------

    /// The three key shapes each resolve their line differently, and the
    /// `Cold` stable case is the one that goes wrong quietly: dividing the
    /// **packed** key by 64 would put `base << 42` into the line index, so
    /// base 28 / imm 0 and base 0 / imm 0 would land on different lines
    /// while base 28 / imm 0 and base 27 / imm (1 << 42 × 64) would collide.
    #[test]
    fn line_identity_per_memref_class() {
        assert_eq!(LineId::of(MemRef::stack(0), 64), Some(LineId::Stack(0)));
        assert_eq!(LineId::of(MemRef::stack(63), 64), Some(LineId::Stack(0)));
        assert_eq!(LineId::of(MemRef::stack(64), 64), Some(LineId::Stack(1)));
        // The base register is a tag component, never arithmetic.
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
        // The naive `packed / 64` derivation this rejects.
        let packed = MemRef::cold_stable(28, 0).key;
        assert_eq!(
            packed / 64,
            (28u64 << 48) / 64,
            "packed/64 folds in the base"
        );
        assert_ne!(packed / 64, 0);
        // A `Cold` unique key resolves to no line at all.
        assert_eq!(LineId::of(MemRef::cold_unique(0), 64), None);
        assert_eq!(LineId::of(MemRef::cold_unique(9), 64), None);
    }

    /// The geometry the model is built on: 64 KiB / 64 B / 4-way L1D is
    /// **256 sets**; 512 KiB / 8-way L2 is 1024; the swept effective L3
    /// (1 MiB pinned) / 16-way is 1024.
    #[test]
    fn set_counts_come_from_the_profile_geometry() {
        let t = table();
        let s = state(&t);
        assert_eq!(s.set_counts(), (256, 1024, 1024));
        // At the sweep's high end the whole 2 MiB L3 is available.
        let point = SweepPoint::pinned(&t).with("effective_l3_bytes", 2 * 1024 * 1024);
        let big = MemState::new(&t, &point);
        assert_eq!(big.set_counts(), (256, 1024, 2048));
    }

    // --- reuse distance -----------------------------------------------------

    /// **Reuse at distance 1 versus beyond L1D capacity selects different
    /// leaves.** 1024 distinct lines exactly fill the 64 KiB L1D; the
    /// 1025th evicts the LRU line, so the oldest line's reuse is no longer
    /// an L1 hit.
    #[test]
    fn reuse_at_distance_one_and_beyond_capacity_select_different_leaves() {
        let t = table();
        let mut s = state(&t);
        // Distance 1.
        assert_eq!(s.access(&load(0)).level, MemLevel::Compulsory);
        let near = s.access(&load(0));
        assert_eq!(near.level, MemLevel::L1dHit);
        assert_eq!(near.latency, 4);

        // Beyond capacity: touch every one of the 1024 L1D lines after it.
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

    /// **A 5-way conflict pattern inside capacity still misses on 4-way.**
    /// This is the oracle that proves associativity is live rather than
    /// decorative: five lines is 320 B against a 64 KiB cache, so no
    /// capacity term can explain the miss — only the 4 ways of one set can.
    #[test]
    fn five_way_conflict_inside_capacity_still_misses_on_four_way() {
        let t = table();
        let mut s = state(&t);
        // L1D has 256 sets, so lines 0, 256, 512, 768, 1024 all index set 0.
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
        // The control: five lines that do *not* collide stay resident.
        let mut s = state(&t);
        for k in 0..5u64 {
            s.access(&load(k * 64));
        }
        assert_eq!(
            s.access(&load(0)).level,
            MemLevel::L1dHit,
            "the same five-line working set without the conflict must hit"
        );
        // And the footprint really is trivial against capacity.
        assert!(5 * 64 < 65536);
    }

    /// An L1 miss that hits L2 takes the **11-cycle** path, and nothing
    /// else: L2 is strictly inclusive of L1D, so there is no victim path.
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
        // The line is back in L1D afterwards (refill, not a victim buffer).
        assert!(s.l1d_resident(LineId::Stack(0)));
    }

    /// Inclusion needs no back-invalidation, and that is a property of the
    /// geometry rather than an omission: every line of one L2 set maps into
    /// one L1D set, whose 4 ways are exhausted long before the L2 set's 8.
    #[test]
    fn l2_eviction_can_never_strand_a_line_in_l1d() {
        let t = table();
        let mut s = state(&t);
        let (l1_sets, l2_sets, _) = s.set_counts();
        assert_eq!(l2_sets % l1_sets, 0, "L2 sets must partition over L1D sets");
        // Nine lines in one L2 set (stride = l2_sets lines).
        let stride = l2_sets as u64 * 64;
        for k in 0..9u64 {
            s.access(&load(k * stride));
        }
        // Line 0 has been evicted from L2 — and it left L1D first.
        assert!(!s.l2_resident(LineId::Stack(0)));
        assert!(
            !s.l1d_resident(LineId::Stack(0)),
            "a line evicted from L2 must already be gone from L1D"
        );
    }

    /// The DRAM leaf is reachable: a line that was resident and has been
    /// evicted from all three levels takes it. 16 lines in one L3 set is
    /// enough, because those 16 lines are also 16 deep in one L2 set and
    /// one L1D set.
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

    /// A compulsory reference is charged at its class's home level — L2 for
    /// a frame slot, L3 for a cold base — never at DRAM. The module docs
    /// carry the reasoning; this pins the numbers.
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
        // Reuse of either is an L1D hit.
        assert_eq!(s.access(&cold(28, 8)).level, MemLevel::L1dHit);
    }

    /// A `Cold` unique reference resolves to no line, so it never reuses
    /// anything and never grants a reuse.
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
        // Even the same sequence number twice: no line, so no hit.
        assert_eq!(s.access(&uniq(0)).level, MemLevel::Unresolved);
        // And an untagged access is the same path.
        let untagged = EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[0]);
        assert_eq!(s.access(&untagged).level, MemLevel::Unresolved);
    }

    // --- the store buffer ---------------------------------------------------

    /// **Forwarding is matched per slot, not per line.** Eight 8-byte spill
    /// slots share one 64 B line, so a line-grained buffer would hand the
    /// forwarding discount to every load of a *neighbouring* slot. Pinned
    /// at the bracket's low end, where the two paths are numerically
    /// distinguishable (forwarding 1 vs `lat_l1d_hit` 4) — at the pinned
    /// point they are both 4 and the assertion would be vacuous.
    #[test]
    fn forwarding_matches_the_slot_and_not_its_line() {
        let t = table();
        let lo = SweepPoint::pinned(&t).with("store_to_load_forwarding", 1);
        let mut s = MemState::new(&t, &lo);
        // Slot 8 and slot 16 are the same 64 B line.
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
        // The same slot still forwards, and is the cheaper path here.
        let same = s.access(&load(8));
        assert_eq!(same.level, MemLevel::Forwarded);
        assert_eq!(same.latency, 1);
    }

    /// An unresolved store still **occupies** a buffer entry. Occupancy is
    /// what bounds forwarding, and a real store buffer queues stores, not
    /// known addresses — so `depth` unique-key stores drain a resolved one.
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

    /// **A store-then-load of the same slot uses the forwarding path, not
    /// the L1 path.** At the pinned point both cost 4 — the bracket's high
    /// end is `lat_l1d_hit` on purpose — so the *latency* cannot tell them
    /// apart and the verdict is what the oracle reads. Moving the swept
    /// dimension then separates them numerically, which is the second half
    /// of the assertion: the term is genuinely swept and pins nothing.
    #[test]
    fn store_then_load_forwards_rather_than_taking_the_l1_path() {
        let t = table();
        let mut s = state(&t);
        assert_eq!(s.access(&store(8)).level, MemLevel::Buffered);
        let fwd = s.access(&load(8));
        assert_eq!(fwd.level, MemLevel::Forwarded);
        assert_eq!(fwd.latency, 4, "pinned at lat_l1d_hit");

        // The control: a load of a line no store wrote, at the same reuse
        // distance, takes the L1 path.
        let mut s = state(&t);
        s.access(&load(8));
        assert_eq!(s.access(&load(8)).level, MemLevel::L1dHit);

        // Swept: nothing about forwarding is pinned.
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

    /// The buffer is finite, and draining it puts the load back on the
    /// cache path. Depth is derived from `[pipelines]`, not invented.
    #[test]
    fn store_buffer_is_bounded_and_draining_returns_the_load_to_cache() {
        let t = table();
        let s = state(&t);
        let depth = s.store_buffer_depth();
        assert_eq!(depth, 4, "cap_l_each (2) x the pipes port_l names (2)");
        let mut s = state(&t);
        s.access(&store(0));
        // `depth` further stores to distinct lines push line 0 out.
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

    /// The behaviour this replaces: pre-M20 a store to a key **invalidated**
    /// it so the reload missed. Under a store buffer the reload forwards, so
    /// the reload is cheaper than it was — recorded here as an assertion so
    /// the change is a fact rather than a claim.
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

    // --- epoch boundaries ---------------------------------------------------

    /// A call or an SP adjust ends L1 reuse but leaves L2/L3 alone, so the
    /// reload is the 11-cycle path rather than a fresh compulsory miss or a
    /// DRAM access.
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
        // The L2 path refills L1D — there is no victim path.
        assert!(s.l1d_resident(LineId::Stack(0)));
    }

    // --- monotonicity -------------------------------------------------------

    /// Monotonicity at the pinned point: a **dead** access — a store to a
    /// slot nothing loads, or a load of a line nothing touches again — never
    /// lowers any later access's charge. It can only add pressure, and
    /// pressure only ever removes hits.
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
        // And the reused line's own verdict is unchanged.
        let mut s = state(&t);
        s.access(&load(0));
        s.access(&store(1 << 20));
        s.access(&load(1 << 21));
        assert_eq!(s.access(&load(0)).level, MemLevel::L1dHit);
    }

    // --- the `working_set_surcharge` decision -------------------------------

    /// **Why `[transitional] working_set_surcharge` is deleted rather than
    /// kept.** It charged `(distinct keys in an 8-entry window − 4) × 2`.
    /// Real reuse distance charges the same direction — more distinct lines
    /// is dearer — through the mechanism the machine actually has, and by a
    /// larger margin: an extra distinct line costs `lat_l2 − lat_l1d_hit` =
    /// 7 (a compulsory miss instead of a hit) against the surcharge's 2. So
    /// the surcharge adds no ranking information reuse distance does not
    /// already carry, and where the two disagree the surcharge is charging
    /// for pressure the geometry says does not exist: five live lines in a
    /// 64 KiB 4-way cache conflict with nothing. Two overlapping footprint
    /// terms is a defect either way (plans/M20.md item F), so it goes.
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
        // The direction the surcharge existed to express, now expressed by
        // the hierarchy: N distinct lines cost strictly more than N
        // accesses to one line.
        for n in [5u64, 6, 8] {
            assert!(
                distinct_lines(n) > one_line(n),
                "n={n}: {} vs {}",
                distinct_lines(n),
                one_line(n)
            );
        }
        // And it scales *per extra line*, by more than the surcharge did.
        let d5 = distinct_lines(5);
        let d6 = distinct_lines(6);
        assert!(
            d6 - d5 >= DELETED_SURCHARGE,
            "an extra distinct line must cost at least the deleted surcharge: \
             {} vs {DELETED_SURCHARGE}",
            d6 - d5
        );
        assert_eq!(d6 - d5, 11, "a compulsory frame line is lat_l2");
        // The disagreement: the surcharge would have charged 5 live lines
        // for pressure, and the geometry says there is none.
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

    /// Every swept leaf really is read through the point, so item J's ∀
    /// sweep moves the memory model rather than only its pinned end.
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
        // `lat_l1d_hit` is T1 and unbracketed, so it is read pinned.
        assert_eq!(MemState::new(&t, &pinned).lat_l1d_hit, 4);
    }
}
