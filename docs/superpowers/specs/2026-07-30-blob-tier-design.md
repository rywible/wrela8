# The blob tier: bulk write-once bytes beside the event log

**Status:** design **PROPOSED** 2026-07-30, awaiting human review. Companion
to
[2026-07-30-event-sourcing-storage-design.md](2026-07-30-event-sourcing-storage-design.md)
(the store), whose S12 defers this pass. Not a plan; `B<n>` ids become real
numbered decisions when a plan is written.

## Why a second tier exists at all

The store's records are fixed-size and its `RECORD_BYTES` is set by the
fattest event variant (store S2). Bulk data breaks that in both directions:
sizing records for a 30 MB save destroys packing for every other event, and
chaining half a million records per save is absurd. So the store caps event
size at build (store S12) and bulk lives here.

Flagship shapes, for scale: save payloads 10–30 MB, screenshots 1–5 MB,
session replays (which are *input logs*, not video — the machine is
deterministic) a few MB per hour of play.

## What a blob is and is not

| Is | Is not |
| --- | --- |
| Write-once, immutable after commit | Appendable, patchable, or overwritable |
| Identified by an opaque `BlobId` | Named, pathed, or arranged in directories |
| Referenced from events by id | Discoverable by enumeration |
| Reclaimed when unreferenced | Garbage-collected by a sweep |

**Naming lives in projections, not in the tier.** A blob has an id and
nothing else; "save slot 2 for title X" is an event, folded into a
projection. This is precisely what keeps the tier from becoming a
filesystem, and it should be normative rather than assumed — the moment the
blob tier learns a name, someone will want a rename, and then a directory.

## The design

### B1. Size-class slot arrays, declared in the image

The tier is N **classes**, each a fixed array of fixed-size slots, declared
in the image and sized at seal:

```wrela
img.blob_class(name=Saves,       slot_bytes=32.MiB, slots=8)
img.blob_class(name=Screenshots, slot_bytes=8.MiB,  slots=64)
```

This is not a novel mechanism — it is the shape `img.pool[T](name=P,
slots=N, max_payload=B)` already has (store 05 §9), applied to disk instead
of DRAM: reserve exact backing, bind the name once, fail closed when full.
Geometry becomes a build output in the report like everything else, so the
disk cost of a class is a number you read at build time rather than discover
in production.

Within a class there is **no fragmentation** — every slot is interchangeable
— so allocation is a bitmap and nothing more. Across classes, waste is
bounded by the class granularity the app chose.

**A blob exceeding its class's slot size fails closed** (`BlobTooLarge`).
There is no chaining in v1. The alternative — a blob spanning K consecutive
slots — reintroduces first-fit allocation and therefore fragmentation, which
is the whole thing this structure exists to avoid. If chaining is ever
needed, that caveat is the reason to think hard first.

### B2. `BlobId` carries class, slot, and generation

```text
BlobId(u64) = (class: u8, slot: u24, generation: u32)
```

Reads are arithmetic — `offset = CLASS_BASE + slot * SLOT_BYTES` — so like
the log, the blob tier has no index.

The generation is the `SlotMap` discipline from 05 §7, applied here for the
same reason: "lookups validate all three fields, so foreign and stale keys
miss instead of aliasing." A reference to a reclaimed-and-reused slot
**misses** rather than silently reading someone else's data. Generation is
the count of times that slot has been allocated, which is derived (B3), not
stored.

### B3. Allocation state is derived, never stored

This is the load-bearing decision.

The blob catalog is a projection over the log (store S12), so **a blob
exists exactly when a committed event references it.** Everything else
follows:

- The free bitmap is a fold, not a structure on disk.
- Slot generations are a fold — the allocation count per slot.
- **A crash mid-write leaves nothing to clean up.** No event was appended, so
  no projection entry exists, so the slot is already free. The orphaned bytes
  are garbage in a slot the bitmap calls empty, and the next writer
  overwrites them.

No allocator journal, no orphan sweep, no free list, no fsck. The same trick
the store plays with projections — don't store state, derive it — pays a
second time.

Two writers picking the same slot after a crash is harmless for the same
reason: neither was committed, so at most one ever gets referenced.

### B4. The one ordering rule, enforced by the type system

**Blob bytes must be durable before the event referencing them is durable.**
Otherwise recovery can surface an event pointing at a slot whose bytes never
landed. This is the only ordering constraint the blob tier adds anywhere in
the system.

It should not be a discipline anyone has to remember. `commit()` returns the
`BlobId` **only after** the blob's own flush resolves — so an app cannot name
a blob in an event before the blob is on the device, because it does not yet
have the id to name. The ordering is unrepresentable to get wrong rather than
merely documented.

### B5. Write path: bounded staging, chunked, bounded in-flight

A 30 MB write cannot be one operation, and it must not stall a 16.6 ms frame.

```text
w = blobs.begin(class=Saves)?          # allocates a slot (in memory only — B3)
w.fill(body: fn(mut Bytes))            # app fills a bounded staging buffer
...                                     # repeat; writer flushes when staging is full
id = await w.commit()                   # flush; resolves to BlobId only when durable
```

The writer owns one bounded DMA staging buffer (order of 64 KiB), so guest
memory is fixed regardless of blob size and each turn is short. Chunks go
through the ordinary `@driver` receipt path — the blob tier introduces no new
I/O mechanism.

**Bounded blob in-flight depth**, so bulk writes never starve the log for
queue slots. A 30 MB blob is ~470 chunks at 64 KiB; the device is busy for
hundreds of milliseconds and the log must still make progress throughout.

A second blk device for bulk traffic is a real option — the machine already
supports it (`tests/golden/boot-blk-two-devices`) — but it is a later,
evidence-gated move. One device with bounded depth first.

### B6. Read path

`read(id, offset, len, into)` — bounded, chunked, arithmetic addressing, no
index. The catalog projection validates the id's generation before any read
issues, so a stale reference fails as a miss rather than reading live data
from a reused slot.

### B7. Reclamation is a refcount projection; there is no GC

A blob is live while some event references it. That is a refcount projection,
and reclamation is therefore **app-driven through events**, not a background
sweep: an event says "save slot 2 replaced," the projection decrements, and
the slot returns to the bitmap.

No sweep, no compaction, no free list, no pause. Since store S10 has v1 never
truncating, the refcount is exact over full history. When truncation
eventually lands, the refcount survives because projections are snapshotted —
which is the same reason the store's other derived state survives.

### B8. Integrity

Each blob carries a digest — FNV in v1, matching the log's checksum choice
and the VMM recorder's `record::digest_hex`; SHA-256 alongside the store's
hash chain later (store S13).

The digest lives in the **referencing event's payload**, not in tier
metadata, so the reference is self-verifying and the digest is durable with
the thing that names it. Four bytes on an event that is already carrying an
8-byte `BlobId` is cheap. Reads verify.

## Interactions with the rest of the system

**Determinism and replay.** Blob writes are ordinary block writes, already
covered by `Completion::digest` and already suppressed-and-diffed under
`--replay`. No new record/replay mechanism, no new divergence class.

**Frame budget.** Same exposure and same answer as the store's snapshot burst
(store latency note): the CPU cost is bounded per turn by the staging buffer,
and the device cost is bounded by in-flight depth. If a recording shows blob
traffic displacing frame-critical I/O, the fix is depth, then a second
device — in that order.

**Truncation.** Blobs are unaffected by log truncation because their liveness
lives in a snapshotted projection rather than in the events themselves. This
is worth stating explicitly: it is the one place where "projections survive,
events do not" produces a *better* outcome rather than a loss.

**The store's non-goals still hold.** Nothing here adds directories, paths,
rename, mutable extents, or a free list.

## Open questions

1. **Class declaration ergonomics.** The app must predict its blob size
   distribution at build time, and a title with an unusually large save fails
   closed at runtime. That is honest and consistent with the machine's "exact
   static memory" philosophy, but it is the sharpest usability edge in this
   design. Is a report-driven utilization warning enough, or does something
   need to be authorable later?
2. **Staging buffer size** — trades turn length against chunk count and
   per-chunk overhead. Wants the same treatment as the store's segment size:
   derived from a declared bound rather than picked.
3. **Whether the digest is mandatory.** Making it optional saves four bytes
   on events that reference blobs the app already trusts; making it mandatory
   means every blob read is verified. Lean mandatory — this is a
   correctness surface, not a performance one.

## Non-goals

Appendable or mutable blobs. Names, paths, directories, rename. Enumeration
or discovery. Chaining slots (v1 — see B1 for why the fragmentation caveat
matters). Background garbage collection or compaction. Deduplication by
content address. A second blk device (v1). Encryption (inherits store S13:
deferred, and blobs would be encrypted alongside snapshots when it lands).
