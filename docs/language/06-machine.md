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

- **3 vCPUs**, always. The image assigns every actor to one of them at
  build time ([04 §3](04-compiler.md)); hosts with more cores run VMM
  threads on the surplus. The count is derived, not arbitrary:
  `vCPUs = flagship core count − 1 housekeeping core` (Pi 5: 4 − 1 = 3).
- ISA baseline: **AArch64 ARMv8.2-A + NEON/ASIMD**, the intersection of
  Cortex-A76 and Apple Silicon. The compiler's one cost model is the A76;
  images simply run faster on M-series.
- No firmware-visible feature discovery: the baseline is the contract, and
  boot asserts it.

## 2. Memory

- One fixed guest-physical layout, published here per machine revision:
  image load base, per-core stacks, device shared-memory windows, and the
  framebuffer region at fixed addresses.
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

The complete, closed device set of machine v1 — each a virtio-family
contract whose driver ships in the stdlib and whose model ships in the VMM:

| Device | Contract |
|---|---|
| `blk` | virtio-blk, split ring, `Flush`, per-queue reset. |
| `net` | virtio-net, split ring. |
| `input` | virtio-input: keyboard, pointer, touch, tablet events. |
| `console` | virtio-console: serial streams, log capture in tests. |
| `entropy` | virtio-rng, recorded under replay. |
| `sound` | virtio-snd: PCM in/out with bounded period buffers. |
| `display` | See §7 — framebuffer push, not a GPU. |
| `clock` | Paravirtual monotonic clock + wall reference; backs `now()`. |

There are no other devices and no device hotplug. A future device is a
machine revision. Appliance authors do not write drivers; the `@driver`
machinery ([03](03-hardware.md)) is how the stdlib drivers are themselves
written and checked.

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
**replay reproduces exactly what the user saw**, and CI can assert
golden-image digests through the VMM.

## 8. Record/replay boundary

The VMM is the recorder. It logs: every device completion and DMA-written
byte range, every vector raise with its consuming checkpoint, every clock
and entropy read, per-mailbox cross-core admission order (the machine's
only scheduling nondeterminism), and digests of every output (block writes,
packets, frames, audio periods). Replay feeds the log from virtual device
models, suppresses real outputs, and diagnoses any divergence. This
implements chapter [04 §10](04-compiler.md) natively rather than as an
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
every device contract, determinism of the recorded boundary, and the
golden-image display tests. The compiler pins the machine revision into the
build identity; the VMM refuses an image built for another revision.
