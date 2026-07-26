# Hardware

The driver surface of the language. A wrela image has one address space;
driver/application separation is typed authority, not privilege rings.

The machine's device set is closed and its drivers ship with the stdlib
([06 §6](06-machine.md)) — appliance authors wire drivers in the image and
call their safe APIs; they do not write drivers. This chapter is how those
stdlib drivers are themselves written and checked: the same safe language,
under the rules below. The virtio contracts follow OASIS VIRTIO 1.2 split
rings as profiled by the machine spec, which also fixes the display
device's software-rendering scanout contract ([06 §7](06-machine.md)).

## 1. Capabilities

Hardware operations require unforgeable resource values minted while the
image binds a declared device to a `@driver`:

- `DeviceCap[D]` — authority over one device instance;
- `Mmio[L]` — a typed register layout derived from that device;
- `IrqCap[V]` — authority over one interrupt vector;
- `DmaPool[P, N]` — a bounded device-reachable pool; and
- target-specific narrow capabilities (queue notifiers, ...).

Their constructors are not source-visible: no address, import, or cast
creates one. The compiler checks provenance transitively — a function that
touches MMIO, DMA, or IRQ state must be reachable through the owning driver's
authority. `@actor` structs cannot hold capabilities in fields, parameters,
messages, or captures; a driver may export safe actor APIs but never raw
capabilities. The device itself is named once, at the image binding
(`img.driver(BlkDriver, device=blk_device)`), the single source of truth.

```wrela
@driver
pub struct BlkDriver:
    irq_regs: Mmio[VirtioIrqMmio]
    queue: VirtQueue[..128]

    init(mut self, take cap: DeviceCap[VirtioBlock],
         take pool: DmaPool[BlockControl, 256.KiB])
        -> Result[unit, BootError]:
        ...
```

## 2. Typed MMIO

Raw integer-address MMIO does not exist in the safe language. A driver or
sealed protocol partitions its claim into declared, non-overlapping layouts:

```wrela
@layout(mmio, endian=little)
struct VirtioIrqMmio:
    @offset(0x060) interrupt_status: ReadOnly[u32]
    @offset(0x064) interrupt_ack: WriteOnly[u32]
```

The compiler and target ABI check width, alignment, non-overlap, bounds, and
endianness. Minting a layout consumes those byte ranges from the claim; two
live layouts can never alias a register. For virtio, the sealed transport
protocol owns the initialization/queue/status/config partition; the driver
maps only what its ISR needs (the interrupt partition above). MMIO access is
volatile and effectful: never elided, merged, or reordered across target I/O
boundaries. Physical addresses are obtainable only by sealed DMA/queue APIs
for their own pools.

## 3. DMA

`@layout(kind, ...)` is the one exact-bytes mechanism, with four kinds:
`dma` (device-visible memory, checked against the target ABI), `mmio`
(register maps, §2), `wire` (persistent/network bytes — exact encoding
independent of any target, no capabilities or target-dependent fields
inside), and `runtime` (the machine's own tables, §3.1). For every
`@layout` type the compiler reports exact size, offsets, padding, and
endianness, and rejects anything implicit or target-dependent.
Byte decoding (`Bytes.read_wire[W]`) exists only for `wire` layouts.

Two kinds of DMA memory:

**Transfer payloads** are `own[P] T` where `P` is a device-bound DMA pool and
`T` is `@layout(dma)`. While the CPU owns the handle, code reads and mutates the
payload normally. Publishing to the device consumes the handle; the source
owns a `Receipt[own[P] T]` instead, and touching the payload while the device
owns it is unrepresentable:

```wrela
receipt = queue.publish(operation=take prepared)
completion = await receipt          # ownership returns with the completion
buffer = take completion.payload
completion.status?
```

**Shared control memory** (descriptor tables, rings) is `DmaShared[P, L]`:
permanently shared, exposing only field-wise typed operations that carry the
target's volatile/cache/ordering semantics. It cannot be read as bytes or
lent as a plain value.

A DMA pool is declared in the image with size, purpose, device reachability,
alignment, and coherency policy. All memory a device can reach originates
from its bound pools; targets with an IOMMU map only those pools. Ordering
never appears as freestanding fences in driver source: the sealed queue
operations (`write_descriptors`, `publish_available`, `load_used_index`,
`notify_queue`, ...) carry the normative order — payload writes before
publication, publication before doorbell, acquire before reading used
entries, ownership return only after acquire/invalidate work.

### 3.1 Runtime layouts and placed statics

`@layout(runtime)` describes the machine's own tables — the structures the
scheduler, mailboxes, turns, and groups are built from. Every rule above
applies to it unchanged: exact size, offsets, padding and endianness, all
reported, nothing implicit and nothing target-dependent. It adds one
allowance the other three kinds do not have: a field of a `runtime` layout
may be another `@layout(runtime)` type, or a fixed-length array of one, so
a table is one declaration rather than a hand-computed set of offsets. A
`runtime` layout is not device-visible — no device reads or writes it, so
it is neither a `dma` payload nor an `mmio` register map — and it carries
no capability.

`@placed(ADDR)` binds one declaration to the fixed comptime address
`ADDR`, so the runtime's tables live where the machine's own code expects
them and the address is a checked build output rather than a convention.
It is legal on exactly one construct: a module-level `static` of a
`@layout(runtime)` type, with at most one placed static per address. It is
legal nowhere else — not on a field, a function, a parameter, a local, or
any other declaration. **Revision 0.1 has no `static` declaration**
([02 §13](02-language.md)), so the construct `@placed` attaches to does not
exist in the language yet and the compiler refuses the attribute wherever
it is written. The rule is stated here rather than held back with the
construct, because it is the reason the `runtime` kind exists; the shape it
will take is:

```text
@layout(runtime, endian=little)
struct TurnArea:
    state: u32
    waiter: u32

@layout(runtime, endian=little)
struct TurnTable:
    rr_cursor: u64
    turns: [TurnArea; N_TURNS]

@placed(0x40500000)
static TURNS: TurnTable
```

## 4. Queues

A queue API reserves complete operations, not raw descriptors:

```wrela
permit = self.queue.reserve_proven(descriptors=3)   # build-proven capacity
operation = self.queue.prepare_block(permit=take permit, header=..., payload=take buffer, ...)
receipt = self.queue.publish(operation=take operation)
```

`reserve_proven` exists only when whole-image analysis proves every admitted
handler a complete unit (three direct descriptors in a 128-deep queue means
at most 42 in flight — the compiler computes it). For runtime backpressure, a
generated proxy waits for capacity *before* admitting the handler; a handler
never awaits a permit its own bottom half produces.

The queue tracks each operation by ID, slot generation, and reset epoch —
none of which ever wrap; exhaustion retires the slot or forces reset/fatal
rather than reusing an identity. Completion is keyed by the device-reported
ID and validated against generation and epoch; stale, duplicate, or unknown
IDs are driver faults, never unchecked indexes.

## 5. Receipts

`Receipt[P]` is the one sealed resource state machine for work published to a
device and resolved later:

```text
Receipt[P] = Submitted | Committed | Resolved(P) | Recovery
```

Before its typed commit boundary, any failure returns `P` unconditionally.
After commit, recovery follows the protocol's quiescence path or reports
`OutcomeUnknown`. A receipt resolves exactly once; dropping one is illegal in
every state. The compiler verifies a receipt implementation against this
machine.

The handoff needs no annotation because the signature is fully determining:
**any public synchronous `@driver` method with exactly one `take p: P`
parameter and result `Receipt[P]` receives the handoff calling convention**,
verified by the compiler and displayed by tooling. Admission commit
atomically moves `P` into the message *and* installs the caller-owned
receipt, before the handler runs. The handler's `return queue.publish(...)`
or `return queue.reject(payload=take p, ...)` transitions that pre-existing
pair; abandonment before either transition routes payload and producer
through supervised recovery. The driver can therefore accept the next
submission while hardware completes the first — its bottom-half turn drains
completions and resolves receipts without re-entering anyone.

## 6. Interrupts

The ownership unit is a **vector**: exactly one handler per vector, possibly
several vectors per driver (one per queue, plus configuration). The wrela
machine's vectors are paravirtual — shared-memory pending words delivered
only at compiler-emitted checkpoints, with no emulated interrupt controller
([06 §4](06-machine.md)) — so delivery is deterministic and identical on
every host. The vector table is generated from the image graph; source
cannot bind an unowned vector.

An interrupt handler is a plain `fn` bound to a vector at image/driver
wiring (`irq.bind(self.on_queue_irq)`). The binding — not a keyword — makes
the compiler restrict the function's transitive effects to the ISR set:

- read/write its device's typed MMIO and acknowledge its source;
- read/write that driver's `InterruptCell[T]` state;
- call helpers whose inferred effects fit the same set; and
- `wake(...)` a statically bound task.

It cannot allocate, await, block, call another actor, touch device-owned DMA
payloads, drain unbounded work, use floating point, or format. An
interrupt-status register is untrusted: the handler masks it against the
declared handled bits, ignores undefined bits, and never writes undefined
bits to an acknowledge register.

```wrela
fn on_queue_irq(self):                # ISR via irq.bind in driver init (see prose)
    status = self.irq_regs.interrupt_status.read()
    handled = status & (INT_VRING | INT_CONFIG)
    if handled != 0:
        self.pending.fetch_or_release(handled)
        self.irq_regs.interrupt_ack.write(handled)
        wake(BlkDriver.drain_used)
```

`InterruptCell[T]` is the sole ISR/ordinary-code channel: `load_acquire`,
`store_release`, `swap_acquire`, `fetch_or_release`, and friends,
interrupt-atomic with respect to every vector that may touch the cell. A
plain field is not a communication channel. Parking on a level predicate uses
the runtime's mask–arm–recheck primitive, so a wake before, during, or after
publication is never lost. Revision 0.1 masks the current vector on entry and
forbids nesting; the compiler reports the maximum interrupt-masked interval.

All substantive work happens in the driver's **bottom half** — a
high-priority `@task` turn that consumes the level signal, drains a bounded
number of completions, validates IDs/generations/epochs/lengths, resolves
receipts, and re-wakes itself if work remains.

## 7. IRQ, poll, and hybrid modes

Driver mode is a const generic (`BlkDriver[DriverMode.Irq]`) whenever it
changes the ISR/actor/effect graph — never a runtime option, so the analyzed
graph is the emitted graph. A poll build eliminates the ISR and vector
entirely and runs its bounded poll task every loop pass (no idle sleep while
mandatory pollers exist); callers and receipts are identical in every mode.
Hybrid policies are library code.

## 8. Untrusted device data

wrela has one **marked value** mechanism — a sealed wrapper that gates use
of a payload until an explicit, typed transition — with three instances:
`Untrusted[T]` (must be checked before use as a bound), `Validated[F, T]`
(proof a declared parser ran), and `Secret[T]` (must never leak into the
image, logs, or control flow; [02 §12](02-language.md)). One mechanism,
three policies; no other gating wrappers exist.

Device **control** values that can influence an index, length, allocation, or
bound arrive as `Untrusted[T]` and cannot be used until checked-narrowed:

```wrela
reported: Untrusted[usize] = completion.written_len
written = reported.checked_le(buffer.capacity())?
```

Device-written **payload** bytes become ordinary data once the protocol
validates the reported extent — but their format still needs its own
validation (a filesystem checks magic numbers and extents; parsers return
checked values). `Validated[F, T]` ([05 §6](05-library.md)) is the wrapper an
API can require as proof that a declared parser ran. This applies even to
hypervisor-backed virtio: devices can also be hardware.

## 9. Protocol states, cancellation, reset

Device bring-up is a typed state chain (for virtio: `Reset -> Acknowledged ->
DriverClaimed -> FeaturesNegotiated -> FeaturesAccepted -> QueuesConfigured ->
Running`); publication requires `Running`, and reset consumes it, producing a
new epoch that invalidates all prior receipts. Each fallible transition
**consumes** its input state and, on failure, routes the underlying
capability to its restart provision internally — so a driver `init` is a
straight line of `?`-propagating consuming calls with no cleanup
choreography of its own. The image declares required features
(`img.device`, [05 §9](05-library.md)); boot still negotiates the real
device.

Once published, a virtio request cannot generally be retracted, so cancelling
in-flight work is a driver protocol, not a dropped future: the receipt moves
to a generated high-priority recovery turn on the owning driver, affected
regions and DMA slots are quarantined, per-queue reset (when negotiated) or
full reset establishes quiescence, and only then is memory reclaimed and the
cancellation resolved. A reset may fail sibling requests; every affected
owner gets the same epoch-carrying reset error. After a reset, a write may
have happened:

```wrela
enum CompletionOutcome:
    Completed
    NotCompleted
    Unknown
```

Source must not auto-retry a non-idempotent operation on `Unknown`. If the
target cannot prove quiescence within its declared bound, it quarantines the
device and pool under an explicit policy or goes target-fatal; it never
reclaims possibly device-owned memory.

## 10. Residual trusted base

Trusted: the compiler and generated code; the wrela VMM and its device
models; the host kernel; and the sealed capability constructors. All but
the host kernel are in-house ([06 §9](06-machine.md)); there is no
third-party firmware or vendor blob in the path. The language keeps apps
away from MMIO and safe drivers away from arbitrary memory; it cannot make
one address space as fault-contained as separate hardware processes, and
documentation must preserve that distinction.
