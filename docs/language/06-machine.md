# The wrela machine

wrela targets exactly one machine — and it is a machine we design, not one
we inherit. This chapter is the versioned contract (**wrela-machine-v1**)
between the compiler, the stdlib drivers, the conformance suite, and every
implementation of the machine. Two implementations exist by design:

- **QEMU** (bootstrap only): the `virt` platform configured to present this
  contract, used until the wrela VMM boots images, then retired.
- **The wrela VMM** (the product): a Firecracker-class userspace VMM,
  written in Rust, running on Linux/KVM (the Raspberry Pi 5 flagship host)
  and on macOS/Hypervisor.framework (Apple Silicon development and Mac
  hosts). It is codesigned with the language: the compiler's image report is
  its configuration, and its device models are the other half of the stdlib
  drivers.

Nothing below imitates real hardware unless imitation is useful. The
machine only needs to be what the stdlib drivers speak.

## 1. CPU

- **N vCPUs**, sealed by the image. The image authors
  `Image(name=..., target=..., cores=N?)` with optional comptime `usize`
  `N` ≥ 1 (default **1**); the compiler assigns every actor to one of
  cores `0 .. N-1` at build time ([04 §3](04-compiler.md),
  [05 §9](05-library.md)); the report digests N into build identity; the
  VMM creates exactly those N vCPUs. Core count is an image fact, not a
  machine revision and not ambient discovery. The contract publishes no
  core-count maximum; a soft page-packing ceiling may refuse pathological
  N the same way other layout ceilings fail closed, but that is not "the
  machine has K cores." The VMM refuses at boot if the host cannot
  provide N vCPUs (short host is a VMM error, never a guest probe — §3).
  The flagship housekeeping arithmetic (`host cores − 1`) remains advice
  for choosing N on a Pi 5, not a contract constant the image must equal.
- ISA baseline: **AArch64 ARMv8.2-A + NEON/ASIMD**, the intersection of
  Cortex-A76 and Apple Silicon. Conforming hosts run the same sealed
  words; images often run faster on M-series. The flagship **A76** is the
  product/backend microarchitecture story for proofs and schedules that
  need one. Separately, the compiler's **proxy-cycle ranking** model
  ([04 §5](04-compiler.md)) scores that baseline ISA **emission** against
  a published-record-derived model of the flagship itself — Cortex-A76 /
  BCM2712 / Raspberry Pi 5, profile `a76-pi5`, the only profile there is
  — with documented pipelines and dispatch constraints, real cache and
  TLB geometry, and swept ranges wherever the record is silent. Rank
  direction only: it is still **not absolute wall time on any host**, and
  its specificity to this board is the point rather than a limitation,
  since an emission tuned where capacity and issue pressure bind first is
  not made worse by a larger machine's slack.
- No firmware-visible feature discovery: the baseline is the contract, and
  boot asserts it.

## 2. Memory

- One fixed guest-physical layout, published here per machine revision:
  image load base, per-core stacks, device shared-memory windows, and the
  framebuffer region at fixed addresses.
- Within the sealed image, the actor runtime-tables section (`rtdata`)
  begins at a fixed packing base **`RTDATA_BASE = IMAGE_BASE + 2 MiB`**
  (`0x4054_0000`). `entry` / `code` / `rodata` / `abort` / `checkpoint`
  pack below that base; layout steers the cursor there rather than
  bump-allocating after code. A build whose packed sections would overrun
  `RTDATA_BASE`, or whose `rtdata` alone exceeds **`RTDATA_SIZE_MAX`
  (256 KiB)**, fails closed at layout — mailbox capacity is otherwise an
  unbounded image-authored integer. This is an in-image packing constant;
  guest-visible pages, stacks, and entry are unchanged, so it is not a
  machine-revision bump.
- The flagship profile is **1 GiB**. The image report's peak-memory ceiling
  must fit the profile or the build fails; the VMM allocates exactly the
  reservation (hugepage-backed, never overcommitted, no balloon device).

## 3. Boot

Direct boot, no UEFI, no firmware, no discovery:

1. the VMM reads the sealed image and its report, validates digests, and
   preconfigures every device, queue, and shared-memory window the report
   declares — device topology is a *build output*, not a probed fact;
2. it loads the image at the fixed base, zeroes the declared reservations,
   points `x0` at the machine-info page (machine revision, wall-clock seed,
   provisioned secrets channel), and starts vCPU 0 at the image entry;
3. the entry installs per-core state, releases the other vCPUs, runs typed
   driver and actor initialization in image dependency order, opens
   mailboxes atomically, and enters the per-core event loops.

Cold boot is a design property: there is nothing to negotiate.

## 4. Interrupts: paravirtual, checkpoint-injected

The machine has **no emulated GIC**. Interrupt hardware exists to preempt
CPUs that might be anywhere; wrela code is only ever *somewhere* — at a
compiler-emitted checkpoint or parked. So:

- each virtual vector is a word in a per-core shared-memory page; the VMM
  raises a vector by a store-release plus a wake of the target vCPU
  (KVM: kick/`sev`; macOS: vCPU exit-resume);
- the guest observes vectors **only at checkpoints and parks**, via the
  same mask–arm–recheck protocol `InterruptCell` already specifies — a
  wake between test and park cannot be lost;
- ISR-bound functions ([03 §6](03-hardware.md)) run at the next checkpoint
  boundary of the owning driver's core, never between arbitrary
  instructions.

Consequences: both host backends are symmetric (no in-kernel vGIC needed on
macOS), the worst of ARM virtualization is deleted, and **injection points
are deterministic by construction** — record/replay logs which checkpoint,
not which instruction. Latency is bounded by the checkpoint bounds the
compiler already proves for every loop ([04 §1](04-compiler.md)).

## 5. Doorbells and exits

Hot paths never trap:

- guest→host notification is a shared-memory doorbell word per queue plus
  one host-visible wake; the VMM's I/O threads poll hot doorbells on their
  own host cores and arm wakes when idle;
- MMIO-shaped register access ([03 §2](03-hardware.md)) exists only on
  setup/reset paths, where a trap is fine;
- the compiler may batch doorbells using its queue-depth and budget
  knowledge; the report states expected exit rates per device.

Idle is codesigned for power: when a core's scheduler has no ready work it
parks with its **next deadline** written to the machine-info page; the VMM
sleeps the vCPU thread until that deadline or a wake, letting the host
reach deep idle states. Exact static memory plus tickless deadline sleep is
the power story.

## 6. Devices

### Machine v1

The complete, closed device set of machine v1. Every row is a **device**
(VMM model + report/conformance, and record/replay where applicable).
Only some devices earn a guest `@driver` ([03](03-hardware.md)); the
rest expose a sealed thin guest surface. Say **virtio** only where the
shipped contract is virtio. There are no other devices and no device
hotplug in machine v1. A future device is an additive machine revision
under [§10](#10-conformance).

The stdlib ships `@driver`s for queue devices under the reserved alias
`drivers` ([02 §2.1](02-language.md#21-the-build-root)); thin
devices use sealed language/runtime surfaces (`now()`, console ring
helpers, and `entropy[N]()`). Appliance authors typically do not write
`@driver`s; the `@driver` machinery is how those stdlib queue drivers are
themselves written and checked.

#### Thin device contracts (no `@driver`)

| Device | Contract | Guest surface |
|---|---|---|
| `clock` | trapping monotonic MMIO (not virtio) | `now()` / `core.time` |
| `console` | fixed console-ring + VMM drain (**not** virtio-console) | runtime / optional `core` helpers |
| `entropy` | recorded entropy source (**not** virtio-rng) | `entropy[N]()` |

#### Queue device contracts (`@driver` in `stdlib/drivers/`)

| Device | Contract | Guest surface |
|---|---|---|
| `blk` | **virtio-blk**, split ring, `Flush`, per-queue reset | `drivers.blk` |
| `input` | pixels rung (queue/`@driver` when scheduled) | `drivers.*` |
| `display` | See §7 — framebuffer push, not a GPU; pixels rung | `drivers.*` |

### Future revisions

The following contracts are preserved as written for a future additive
machine revision ([§10](#10-conformance)); they are **non-normative until
revised** — outside machine-v1 conformance. When revised they are
expected to be queue/`@driver`-class unless a later design says
otherwise:

| Device | Contract |
|---|---|
| `net` | virtio-net, split ring. |
| `sound` | virtio-snd: PCM in/out with bounded period buffers. |

## 7. Display: software rendering, zero-copy scanout

Rendering is pure CPU (NEON) in the guest; the display device only moves
pixels out:

- framebuffers are **blob resources**: guest-owned pages the VMM maps and
  scans out directly — `transfer` is a no-op, `flush` is one doorbell per
  frame;
- scanout accepts a **tile list** (scatter-gather): the compositor's
  workers fill disjoint `own[Tiles] Tile` buffers on their assigned cores
  and the frame actor submits the assembled list — parallel rendering is
  ownership transfer, never shared mutation, and the VMM does the gather;
- the device delivers a vsync event on the frame vector; the flagship mode
  is 1080p60 (4K is a stretch profile, not the baseline);
- host backends: DRM dumb buffers / Mesa-V3D present on the Pi host, a
  Metal layer on macOS.

Because every pixel is CPU-computed and every flush is a recorded output,
**replay reproduces exactly what the user saw**, and the conformance suite can
assert golden-image digests through the VMM.

Pixels adds two image-internal regions after ordinary runtime data:
`frameprog` contains immutable, 64-byte-aligned frame programs and
`pixelsdata` contains zero-initialized, 64-byte-aligned renderer state. These
regions are compiler-placed guest memory, not devices or host-visible renderer
state, and do not alter display ownership or queue semantics.

An `Image.renderer` declaration creates a generated `Renderer[P]` actor and
deterministically placed internal workers. The guest actors alone consume the
frame program, construct and shade pixels, fill disjoint guest-owned tiles,
and submit the complete display list. The VMM validates and scans out those
bytes without rendering or modifying a pixel. A renderer failure leaves the
previous complete framebuffer visible and submits no partial frame.

## 8. Record/replay boundary

The VMM is the recorder. It logs: every device completion and DMA-written
byte range, every vector raise with its consuming checkpoint, every clock
and entropy read, per-mailbox cross-core admission order (the machine's
only scheduling nondeterminism), and digests of every output (block writes,
packets, frames, audio periods). Replay feeds the log from virtual device
models, suppresses real outputs, and diagnoses any divergence. This
implements chapter [04 §9](04-compiler.md) natively rather than as an
optional profile.

## 9. Hosts, packaging, trust

- **Flagship**: Raspberry Pi 5 (4×A76, 1 GiB usable for the guest). The
  shipped artifact is a triple — a minimal read-only Linux host (kernel +
  VMM, nothing else), the VMM, and the wrela image — all built and
  digest-pinned by the toolchain, so byte-for-byte reproducibility covers
  the whole appliance, and A/B update replaces the triple atomically.
- **Development**: macOS on Apple Silicon via Hypervisor.framework — the
  native daily loop, no emulation; and any Linux/KVM aarch64 box.
- The VMM may build on the rust-vmm crates (plain Rust; consistent with
  the self-contained-toolchain rule) and is jailed Firecracker-style on
  Linux, sandboxed on macOS.
- Trusted computing base, in full: the wrela compiler and generated code,
  the VMM and its device models, and the host kernel (Linux or XNU). The
  first two are in-house and codesigned; there is no third-party firmware,
  bootloader, GPU driver, or blob anywhere in the boot-to-pixel path.

## 10. Conformance

An implementation of machine v1 must pass the machine conformance suite:
boot contract, memory map, checkpoint-injection semantics, doorbell ABI,
every machine-v1 device contract (`blk`, `console`, `clock`, `entropy`,
`input`, `display`), determinism of the recorded boundary, and the
golden-image display tests. The compiler pins the machine revision into the
build identity; the VMM refuses an image built for another revision.

A new device (or a change to an existing device's contract) is an
**additive machine revision**: the prior revision's contracts are
preserved, the new revision's conformance suite grows by the added
contracts, and an image built for an older revision remains loadable.
Contracts listed under [§6](#6-devices) as future-revision (`net`,
`sound`) are preserved as written and are **non-normative until revised**
— outside machine-v1 conformance until a revision that names them.
