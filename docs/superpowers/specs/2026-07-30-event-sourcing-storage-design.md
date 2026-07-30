# Event-sourcing storage: the foundational store of wrela OS

**Status:** design **PROPOSED** 2026-07-30, awaiting human review. Revised
2026-07-30 after review round 1 (S4/S5 timestamp, S9 aggregates, S2 record
sizing, S12 blobs). Not a plan; no milestone is activated by this document
and no decision numbers are allocated here (the `S<n>` ids below become real
numbered decisions when a plan is written). M20 is ACTIVE
([plans/M20.md](../../../plans/M20.md), the A76 ruler) and nothing here
interacts with it.

**Context.** ROADMAP's crash-only failure decision (human, 2026-07-26)
accepted bystander loss and paid for it with "a **durability requirement**
(`Reboot` *presumes* app durable checkpoints via storage — named
dependency, currently unbuilt; conformance goldens pin `Halt` until it
exists)." This design is that dependency. It is also the first consumer of
several surfaces the docs specify and no code implements — see
[Findings](#findings-promises-with-no-implementation).

## Goal

An event-sourcing database as the foundational storage library of wrela OS.
Explicitly **not** a filesystem. Three parts:

1. an append-only log of immutable events on virtio-blk;
2. projections — deterministic folds over a log prefix into bounded,
   resident, typed tables; and
3. queries that target only preformed projections and are no more complex
   than a select-where.

Plus a **blob store** (S12) as a separate, adjacent tier: bulk write-once
bytes that events reference by id and never contain.

Co-design across the guest driver, the VMM device model, and the compiler
is in scope; each is modified where the store needs it.

## Why this shape fits this machine

Not merely "event sourcing is a nice pattern." Four properties are specific
to wrela and are what make the design cheap:

- **No dynamic allocation.** The log is the only structure that grows, and
  it grows on the device rather than in DRAM. Everything resident is
  bounded and build-sized. There is no allocator, no free-space map, no
  B-tree, and no journal-vs-data duality to keep consistent.
- **Determinism.** A fold over an append-only log is deterministic by
  construction, so projections replay exactly and can be goldened directly.
- **Ownership and receipts.** 03 §5's `Receipt[P]` state machine already
  gives exactly-once publication with ownership return; the store needs no
  I/O bookkeeping of its own.
- **The cleverness budget is pre-argued.** ROADMAP already records that for
  storage "the software path is already below the device's noise floor — a
  ~1 µs round trip against a 10–80 µs NVMe read is 1–5%." Dumb-and-correct
  is the permanent answer here, not a v0 stance. Nothing in this design
  needs a profile to justify itself.

## Vocabulary (settled — do not blur)

| Term | Means |
| --- | --- |
| **Log** | The single, globally ordered, append-only sequence of event records on the device. The only truth. |
| **Slot / `seq`** | A record's physical position, strictly monotonic. `offset(seq)` is arithmetic. Slots may be *burnt* (S2), so `seq` is a position, **not** a dense event count. |
| **Event** | One app-declared `@layout(wire)` value stored in one record's payload, size-capped at build (S12). The store never inspects it. |
| **Envelope** | The store-owned framing around a payload (S4). |
| **Projection** | A bounded resident table plus a deterministic fold over the log. Derived data; never truth. |
| **Snapshot** | Every projection's table bytes at one `seq`. This *is* "the state of the log" — the log has no state beyond its contents. |
| **Blob** | Bulk write-once bytes in a separate tier, referenced from an event by `BlobId`. S12. |

There is deliberately **no "aggregate"**. See S9.

## The core design

### S1. One log, fixed-size records, arithmetic addressing

Records are fixed-size. `offset(seq) = LOG_BASE + seq * RECORD_BYTES`.

This is the load-bearing decision; most of the rest follows from it.

- **No index exists.** Reading record N is one read. There is no B-tree, no
  offset table, no metadata to keep consistent with the data.
- **Recovery is a binary search, not a scan.** Probe a midpoint; if its
  checksum validates *and* its `seq` field matches its own position, the
  tail is above, else below. O(log n) boot recovery with zero extra
  machinery. This is why `seq` is in the envelope: without a self-locating
  record you cannot distinguish a valid record of a *previous generation*
  of the log from a valid current one.
- **Batching has no semantic footprint.** Record N occupies the same offset
  regardless of how it was batched, so linger period and size trigger are
  pure performance knobs. Retuning them moves no golden of disk content.
  This property holds *only* while records are fixed-size.

One log rather than per-stream logs: total order is free, replay determinism
is free, there is one writer, and a projection folding over *all* events can
join across event types natively — which per-stream partitioning would make
awkward for no gain (S9).

### S2. `RECORD_BYTES` is a build output, not pinned to a sector

**Revised in review round 1.** The first draft pinned records to 512 bytes
so that one record was exactly one sector, on the argument that a torn write
could then only lose whole records and never corrupt one internally.

That argument was **oversold and is withdrawn.** The checksum already
detects internal corruption, and a record failing its checksum is discarded
— so a torn multi-sector record and a torn single-sector record lose exactly
the same thing: the in-flight record, which was never acknowledged durable.
Recovery's binary search reads "checksum fails" as "tail is below here,"
which is correct either way. One-record-per-sector buys simpler *reasoning*
and a marginally simpler read path, not a different failure model — nowhere
near worth an 8× storage sacrifice on a small-event workload.

So: `RECORD_BYTES` is the smallest power of two fitting
`envelope + max_variant`, constrained to divide or be a multiple of 512.
Small events pack several per sector; fat ones span sectors. The report
prints the resulting **padding factor** either way.

Sub-sector packing costs exactly three things, all small:

1. **A flushed sector is never rewritten.** A torn rewrite would destroy
   records already acknowledged durable. So a flush pads out to the sector
   boundary by **burning** the remaining slots, keeping `offset(seq)`
   exactly arithmetic.
2. **Burnt slots must be distinguishable from never-written ones**, or the
   binary search cannot tell "log continues above" from "log ends below."
   Hence the `kind` byte in S4.
3. **`seq` gains gaps.** Still strictly monotonic, so still the law of log
   order (S5) — just not an event tally.

Self-balancing in practice: under load sectors fill and burns are rare; when
idle you burn slots but write almost nothing.

`SECTOR_SIZE` is 512 and is not ours to choose. Note a gap found while
checking this: **the sector size has no normative home.** It exists only as
a VMM constant (`devices.rs`, "the one sector size this machine's blk device
speaks"); 06 never mentions sectors or 512 at all. A store whose entire
addressing scheme rests on it should not depend on a Rust constant — 06 §6's
`blk` row should state it, with a ledger clause.

### S3. Storage geometry is a build output digested into image identity

`LOG_BASE`, `RECORD_BYTES`, segment size, blob-slot geometry, and each
projection's snapshot extent are computed at `img.seal()` from the declared
event type and projection set, emitted into the report, and digested into
build identity — the same treatment device topology already gets ("device
topology is a *build output*, not a probed fact", 06 §3).

Consequence: an image can only open a disk whose superblock matches its own
schema digest. Mismatch **fails closed**; there are no migrations in v1.

### S4. The record envelope

There is **no canonical `Event`** — the payload is the app's own
`@layout(wire)` enum and the store never inspects it. The store needs
exactly three facts: that it is wire-encodable, that its max encoded size is
computable at seal, and that that size is under the S12 cap.

The envelope is deliberately tiny. Each field pays on every record forever:

| Field | Verdict | Why |
| --- | --- | --- |
| `seq: u64` | **in** | Makes each record self-locating; enables binary-search recovery and distinguishes a stale record from a live one. |
| `checksum: u32` | **in** | Torn-write detection, and the gate on the recovery search. FNV-1a specifically, matching the VMM recorder's `record::digest_hex`, so guest and host speak one digest and can cross-check. |
| `kind: u8` | **in** | `Event \| Burn`. Required by S2's flush padding; leaves room for future record kinds. |
| `len: u32` | **undecided** | Redundant if enum wire encoding is self-delimiting (tag determines variant extent). Decide with the enum encoding; do not include reflexively. |
| `timestamp` | **out** (human, review 1) | See S5. |
| format version | **out** | Superblock's job, once. |
| `prev_seq` | **out, permanently** | Existed only to thread per-aggregate streams. S9 deletes aggregates, so it has no consumer. This closes the design's only format-break risk. |
| blob reference | **out** | A `BlobId` is app payload, not framing (S12). |

### S5. `seq` is the sole law of log order; no timestamp in the envelope

**Revised in review round 1** (human): timestamps were accepted into the
envelope and are now **removed**. Two orderings — one authoritative and one
advisory-but-plausible-looking — is a trap; a projection would eventually
rely on the wrong one. `seq` is strictly monotonic and is the only ordering
anything may use.

Apps that need to know *when* something happened put a timestamp in their
own event payload, pay the bytes only where they want them, and inherit no
ordering ambiguity.

**This removes the store's only dependency on the wall-clock capability**,
which does not exist: 05 §5 states `Instant` is opaque and "never serialized
as wall time" and names wall time as "a separate capability" with no guest
surface; the machine-info page reserves a wall-clock seed at offset 0x20 and
the VMM pins it to `0`
([boot.rs:315](../../../crates/wrela-vmm/src/boot.rs), "deterministic at
M5"). Building it (`wall = seed + monotonic`, seed recorded at boot and fed
from the log on replay) is now an **app-driven, independently schedulable**
item rather than a prerequisite of the store.

### S6. Durability: append is synchronous, flush is a barrier

`append` and `flush` are separate commitments.

```text
append(e) -> Seq     # synchronous, one turn: seq assigned, every projection folded
flush()   -> await   # barrier: everything through the current seq is on the device
```

The projection fold happens **in the append turn**. This gives
read-your-writes automatically and makes projection durability *transitive*
on event durability — the store never writes projection bytes on the hot
path, because the fold is deterministic and the log is truth.

The large consequence: **snapshots stop being a correctness mechanism.** A
snapshot may be stale, torn, or entirely absent and the image is still
correct; worst case you replay more log. Snapshot writes therefore need no
ordering protocol against the log, can be discarded on any checksum
failure, and require no coordination whatsoever — *except* on the
truncation path (S8).

Precision: this gives read-your-writes, not durability. The projection may
be ahead of the disk; it can never be ahead of the log.

Cost to record for when it is revisited: an append costs one fold per
registered projection, synchronously. Ten projections, ten folds. Bounded,
build-known, and priceable in the report — which makes it a measurable
bottleneck if it ever becomes one, at which point the follower design (fold
as a bounded-drain `@task`) is the fallback, at the cost of
read-your-writes.

**Batching.** Appends accumulate and are written on a size trigger plus
drain-to-idle. Idle is free — the bottom half already knows when there is no
pending work — and it gets most of what a linger timer buys: batches form
under load, latency stays low when quiet. A time-based linger is a later
refinement, blocked on guest-side deadline parking (see Findings).

### S7. Projections: signature-determined, registered at image build

The mechanism already exists in this codebase. 03 §5 establishes
**signature-determined roles**: "any public synchronous `@driver` method
with exactly one `take p: P` parameter and result `Receipt[P]` receives the
handoff calling convention." No trait, no annotation — the shape *is* the
contract. Projections use the same device.

```wrela
pub struct OrdersByCustomer:
    table: Table[OrderRow, 1024]
    applied: u64

    pub fn apply(mut self, read e: Event, seq: u64):
        match e:
            case .OrderPlaced(o):    ...
            case .OrderCancelled(o): ...
            case _: pass
```

```wrela
p = img.projection(OrdersByCustomer, store=s)
```

Registration joins 05 §9's existing image-builder intrinsic family, whose
arguments already "must match `A.init`." Because the compiler knows every
projection at seal, the fold dispatch is a straight-line sequence of calls —
build-time known, no trait with one implementation, no vtable, nothing
dynamic. This satisfies the doctrine ban on "traits with one implementation"
and "generic-over-backend seams" directly rather than by exception.

Two properties worth naming:

- **The fold is a pure function**, so projections are the most testable
  thing in the image: comptime `@test` over the fold with no device, no
  actor, no boot. The `stdlib-test` lane already exists and is wired into
  `check`, so a projection's whole state after a fixed event sequence can be
  goldened with no VMM involved.
- **The user writes only the fold.** `Table[Row, N]` is library-supplied and
  carries the query surface.

**Row types must be `@layout(wire)`** (required by S8 for snapshotting).
This reads as a constraint and functions as a guardrail: it makes it
*unrepresentable* for a projection to hold a capability, a resource, or an
owned DMA handle. Projections are derived data by construction rather than
by discipline.

**The query surface** is exactly select-where and nothing more:

```text
t.count(pred: fn(read Row) -> bool) -> usize
t.find_first(pred: fn(read Row) -> bool) -> Option[Row]
t.each_where(pred: fn(read Row) -> bool, body: fn(read Row))
```

Bounded linear scan under `@budget(bound=N)`, matching `List`'s existing
`each` / `lend` / `lend_mut` shape. No joins, no aggregation beyond count,
no planner, no query language. If you want a join, you materialize a
projection — that is the orthodoxy and it is also the dumb-and-correct
answer. Indexes are deliberately **not** built: a bounded scan over a
resident table is precisely the case where the cleverness budget says wait
for a profile, and a declared index on one key field is a small additive
change if one ever binds.

`SlotMap` is *not* the right table type despite being the nearest existing
thing: its keys are minted `(map_id, index, generation)` triples, so it
cannot answer "the row for customer 42."

### S8. Snapshots: the whole log state at one `seq`

| Decision | Choice |
| --- | --- |
| Content | Every projection's table, all at the same `seq`. This is what "snapshot the log" means; the log has no other state. |
| Layout | Fixed extent per projection, sized at seal: `ceil(sizeof(table)/512) × 2` sectors |
| Atomicity | A/B double-buffered; higher generation with a valid checksum wins |
| Header | magic, schema digest, generation, `applied_seq`, checksum |
| Trigger | Segment boundary — a pure function of `seq` |
| Concurrency | Fence appends for a **memcpy** into staging; write staging out asynchronously |
| Staging | One shared buffer sized to the largest projection (snapshots are sequenced) |
| Scope | All projections at once (**burst**, human review 1); no opt-out |

A/B plus checksum plus generation gives crash atomicity with **no superblock
update and no ordering protocol** — nothing elsewhere needs updating. A
snapshot bearing a different schema digest is rejected and that projection
simply rebuilds; this is per-projection, so one bad snapshot poisons nothing
else.

Segment-boundary triggering ties the snapshot to the only thing it exists to
authorize (dropping that segment), is deterministic and replay-safe, needs
no deadline-park dependency, and is self-pacing. Because every projection
snapshots at the same `seq`, the truncation watermark is a **single number**.
Segment size becomes the one knob trading recovery time against snapshot
write amplification.

**Why the fence covers only the memcpy.** A table write is large — a
1024 × 64 B table is 128 sectors — so it cannot happen synchronously in an
append turn. It also cannot be streamed from a live table while appends
mutate it: the result is smeared across seqs, and recovering from it would
require re-applying events to rows that already have them, which is sound
only if every fold is idempotent. Counters are not. So the stall is one
bounded, build-known memcpy — priceable in the report — and never a device
round trip.

### S9. No aggregates

**Revised in review round 1** (human). The first draft carried aggregate
rollups as a concept that "collapses into a projection." The concept is now
**deleted outright**, not collapsed.

The reasoning: the log has no state beyond its contents, so "the state of
the log at `seq` N" is definitionally "every projection at `seq` N" — which
is exactly what S8 already writes. Aggregates are *implied by* whatever
projections an app declares; a projection keyed by customer id **is** the
customer aggregate. The store has no reason to know the word.

Stress-tested against what aggregates buy elsewhere, all of which is
already had or irrelevant here:

| Aggregates normally provide | Status here |
| --- | --- |
| Optimistic concurrency per entity | Irrelevant — one writer, one total order |
| Transactional consistency boundary | Strictly weaker than what we have; every append is atomic against the whole log |
| Bounding rehydration cost | No rehydration exists — no dynamic allocation means live state is bounded and resident by construction |
| Stream partitioning | A net loss: it would prevent projections from joining across event types, which folding over all events gives natively |

Consequences: **no `prev_seq`** (its only consumer was cold-aggregate
rehydration), no stream ids, no per-entity ordering, no per-aggregate
snapshot cadence, and one snapshot format instead of two. The design's only
format-break risk is closed.

### S10. Retention and truncation

Truncation cannot precede snapshots, because a durable snapshot is the only
thing that can authorize one. That sequencing is forced, not chosen. v1
therefore **fails closed with `StorageFull`**; truncation arrives with the
segment-reclaim item.

**The permanent price, stated bluntly.** A snapshot preserves every
*existing* projection's state, not the events. So a **new** projection added
to a running system needs history that truncation destroyed — it can only
start empty and be correct going forward. *You can run forever, but you can
never ask a new question about the past.*

That is the strongest argument for a retention policy that keeps **N
segments beyond** the snapshot watermark rather than truncating right down
to it. Retention depth is not "how much disk we have"; it is *how far back
we can still answer a question we have not thought of yet.* This belongs in
normative text.

**The bill for truncation**, up front: reclaiming segments makes the log
circular at segment granularity, so `offset(seq)` stops being globally
arithmetic and becomes arithmetic *within* a segment given a per-segment
base. Each segment gains a small header (`base_seq`, generation) and recovery
reads those few sectors to build a resident segment table. Still no index and
still arithmetic in the inner loop — but "one linear log that fails closed
when full" was strictly simpler, and this is where we leave it.

Fails-closed may last longer than it sounds: with bulk data in the blob tier
(S12), a console's system-of-record log is plausibly a few hundred MiB over a
decade.

**Rejected permanently.** A *circular log* — destroys arithmetic addressing
and binary-search recovery, the two properties paying for this design. *Key
compaction* (Kafka-style latest-per-key rewriting) — destroys event sourcing
outright: once superseded events are gone you can no longer build an
arbitrary new projection from history, only ones depending on latest-per-key.

### S11. Placement: the store actor pins to core 0 with the driver

Correct instinct, but **not** because of fusion. There is no same-core call
fusion in this compiler: 04 §2 lists "handler fusion" as an as-if allowance
the compiler *may* take, and ROADMAP settles it as a rejected first move with
[plans/M19.md](../../../plans/M19.md) placing fusion under "later spends,"
cleverness-budget-gated. Co-locating will not light up an optimization
because there is none to light up.

The real reason exists today: a cross-core actor call goes through M8's
cross-core rings plus publish/acquire barriers; a same-core call goes through
the local scheduler with neither.

It is also already forced. blk is hard-pinned to core 0 —
`tests/golden/err-placement-virtio-blk-core`: *"plans/M8.md keeps it and its
ISR/bottom half on core 0 until an item deliberately moves them."* So the
store actor takes `core=0` and inherits the pin.

**The trade, stated explicitly:** on a multi-core image this makes core 0 the
storage core, so every append becomes a cross-core hop from the app actor to
the store actor. We are not eliminating a hop, we are relocating it to the
cheaper edge — app→store carries one small event; store→driver carries DMA
buffers and receipts and shares the ISR's core.

### S12. Blobs: in scope, separate tier, own design pass

**Revised in review round 1** (human): in scope, and events carry a pointer
rather than bulk bytes.

Bulk data — save payloads (10–30 MB), screenshots, clips, session replays —
is not event-shaped. Forcing it into a fixed-size record log is catastrophic
in both directions: sizing `RECORD_BYTES` for a 30 MB save destroys packing
for every other event, and chaining half a million records per save is
absurd.

**Events are size-capped at build.** If `envelope + max_variant >
MAX_EVENT_BYTES`, the build fails with "this variant belongs in a blob."
Fails closed, and it is what makes S2's packing win reliable rather than
accidental.

**Events reference blobs by `BlobId(u64)`, not by inline digest.** Thirty-two
bytes of sha256 is a large fraction of a small event; a blob catalog
projection maps id → (extent, len, digest), with integrity still verified on
read. The catalog being a projection keeps it self-consistent with
everything else.

**The risk area, named honestly: reclamation is where this design grows
something allocator-shaped**, which is exactly what the rest of it avoids.
Liveness is fine — a refcount projection, which survives truncation because
projections are snapshotted. But reclaiming variable-size extents means
fragmentation, hence either compaction (a real GC) or a free list (a
filesystem in embryo).

The dumb answer avoiding both: a **fixed-size blob slot array**. N slots of
fixed extent, allocation via a free bitmap that is a projection, large blobs
chain slots. No fragmentation, no compaction, fails closed when full. Wastes
space on small blobs — but blobs are the bulk tier, so that ratio is trivial
next to what S2 saves on records.

It is still not a filesystem: no directories, no paths, no rename, no
mutable extents.

**This section is a sketch, not a design.** Blobs deserve their own pass,
including the chunked async write path (a 30 MB blob must not stall a frame)
and the interaction between slot reclaim and truncation.

### Latency note

A console is a 16.6 ms frame budget and storage must never stall a frame.
Appends and async flushes are fine — that is what S6 does. The snapshot
**burst** (S8) is the exposure: every projection written at one segment
boundary, fenced memcpy plus an I/O burst, is exactly the shape that drops a
frame. Likely resolution is scheduling rather than staggering — snapshot
during vblank, hung off the vsync event the pixels rung will provide — which
preserves the single-watermark simplicity. Flagged rather than settled: the
burst decision was taken before this requirement was in view. Blob writes
(S12) have the same exposure and the same likely answer.

## Findings: promises with no implementation

Each is a surface the docs specify that no code provides, and for which this
store would be the first consumer. These are real costs in any plan, not free
preconditions.

| Surface | Status | Where |
| --- | --- | --- |
| `Bytes.read_wire[W]` | **Specified, zero implementation.** `LayoutKind::Wire` is parsed and validated; no decode path exists anywhere in the compiler. | 05 §6, 03 §3 |
| `@layout(wire)` on enums | **Does not exist** — `@layout` is struct-only; an enum with payloads has no exact byte layout. Needed for the event type; the max-over-variants size it computes is the same number `RECORD_BYTES` and the S12 cap need. | 03 §3 |
| Host-file-backed disk | **Absent.** `BlkDevice` owns `disk: Vec<u8>`, zeroed per boot, no host file; `T_FLUSH` returns `STATUS_OK` and does nothing. | 06 §6, §8 |
| A real blk driver | [stdlib/drivers/blk.wr](../../../stdlib/drivers/blk.wr) is a fixture, not a driver: one 512-byte `DmaBlock`, one in-flight op, a phase counter, `capacity_sectors = 16` hardcoded, no mailbox. | 03, 06 §6 |
| Checksum primitive | No hash of any kind in the stdlib. | — |
| `db` package alias | `core` and `drivers` only; `drivers/` is documented as `@driver` modules **only**, so a plain `@actor` store cannot live there. | 02 §2.1 |
| Normative sector size | **Absent.** 512 lives only as a VMM Rust constant; 06 never mentions sectors. See S2. | 06 §6 |
| Guest-side deadline parking | **VMM side implemented** (`OFF_NEXT_DEADLINE`, `capped_park_deadline_ns`); guest side absent — `wrela-vmm/src/lib.rs` says in as many words that no `.wr` source can exercise a real deadline. Blocks a time-based linger only; not on the critical path. | 06 §5 |
| Wall-clock capability | **Named, no guest surface.** Seed reserved at machine-info 0x20, VMM pins it to `0`. **No longer a store dependency** after S5; needed only by apps that timestamp their own events. | 05 §5, 06 §3 |

## What has to be built

**VMM.** `--disk <path>` with real file backing; `T_FLUSH` becomes a real
fsync; disk sha256 emitted as a report `Input` line (the mechanism already
exists in `ParsedReport::input_digests`) so a boot stays reproducible; under
`--replay`, file writes are suppressed and diffed against recorded digests
(`Completion::digest` already covers block writes, so the recording half is
largely present); boot validates the superblock against the report's schema
digest. Probably also a way to create and size a disk file.

**06 §8 revision.** The substantive normative change. Today "replay ...
suppresses real outputs" is satisfied *by construction* because a block write
has no output. A host file makes block writes genuine outputs, and the
record/replay boundary must say so explicitly.

**Compiler.** `@layout(wire)` end to end plus `Bytes.read_wire[W]`; enum wire
encoding with tag-plus-padded-payload and max-variant sizing; the S12 event
size cap as a build error; storage geometry as build outputs in the report;
`img.projection(...)` joining the 05 §9 intrinsic family; the padding-factor
report line.

**Driver.** [blk.wr](../../../stdlib/drivers/blk.wr) is a **rewrite**, not an
extension: `mailbox=`, the 03 §5 handoff convention
(`pub fn submit(mut self, take b) -> Receipt[...]`), N in-flight tracked in a
`SlotMap` with the bottom half draining and resolving, real read / write /
flush, and the actual `Device.read_capacity_sectors` (which already exists as
a build-time constant). Existing blk boot goldens must be migrated, not
broken.

**stdlib.** A third reserved alias `db` alongside `core` and `drivers` —
loader resolution, a row in 02 §2.1's build-root table, and a charter line in
`stdlib/README.md` so the next person does not put a driver in it. Plus the
store actor, `Table[Row, N]`, and an FNV-1a checksum.

**Docs and ledger.** 06 §8 (outputs boundary), 06 §6 (blk row: flush is a
real barrier; sector size stated normatively), a new 05 section for the store
contracts, 03 §3 (wire enums), 02 §2.1 (`db` alias), and new clauses
throughout. The crash-only durability debt flips off `gap`.

## Sequencing sketch

Not a plan; the shape a plan would take.

1. **Foundations** — wire layouts end to end, enum wire encoding,
   `read_wire`, FNV-1a, `db` alias.
2. **Real device** — VMM file-backed disk, real flush, 06 §8 revision, blk
   driver rewrite, boot-golden migration.
3. **The log** — superblock, envelope, append, batching, burn-slot padding,
   binary-search recovery, `StorageFull`.
4. **Projections** — `img.projection`, `Table`, select-where, comptime fold
   tests.
5. **Snapshots** — A/B extents, segment trigger, fenced memcpy, recovery
   integration.
6. **Later, evidence-gated** — truncation and segment reclaim (with the
   keep-N-segments policy of S10); the blob tier (own design pass); vblank
   snapshot scheduling; time-based linger; wall-clock capability for app
   timestamps.

Realistically two to three milestones through step 5.

## Open questions

1. **Does `len` earn its place in the envelope?** Decide together with enum
   wire encoding — redundant if the encoding is self-delimiting.
2. **`MAX_EVENT_BYTES`.** The S12 build cap. Wants a number; it sets
   `RECORD_BYTES` for everyone and therefore the packing ratio.
3. **Snapshot burst versus vblank scheduling.** The burst decision predates
   the frame-budget requirement; see the latency note.
4. **Segment size**, and **how many segments to retain beyond the watermark**
   (S10). The first trades recovery time against snapshot write
   amplification; the second is how much "asking a new question about the
   past" is worth.

*Resolved in review round 1:* whether to reserve `prev_seq` (no — S9 deletes
its only consumer); whether records pin to a sector (no — S2); whether the
envelope carries a timestamp (no — S5).

## Non-goals

A filesystem. Directories, paths, rename, mutable extents. A query language
or planner. Joins, aggregation beyond count, ad-hoc queries. Indexes (until a
profile). Migrations (v1 fails closed on schema mismatch). Aggregates as a
first-class concept. Per-stream logs. Circular logs. Key compaction. Restart
semantics beyond the existing crash-only policy. Replacing virtio-blk.
Anything requiring a general device framework.
