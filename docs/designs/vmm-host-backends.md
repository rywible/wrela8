# Wrela VMM host backends and Raspberry Pi execution plan

**Initiative boundary:** This V0–V15 program is an independent post-P8R
roadmap. Its presence in the P8R integration tree is not implementation of a
P8R task and must not be cited as P8R acceptance evidence.

**Status:** IMPLEMENTED — Tasks V0–V15 are represented in the compiler,
machine contract, portable VMM, HVF/KVM adapters, Forge and Rasputin tooling,
packaging checks, deterministic corpora, and the standing physical-evidence
drift lock. This document records the host-backend boundary, the audited
Linux/KVM dependency exception, the Raspberry Pi development and measurement
lane, and the presentation boundary; chapter 06 remains the normative
guest-visible contract.

**Repository basis:** the P8R milestone close on branch `P8.5-impl`. The
working tree at the basis contains the P8-era Linux/KVM and DRM prototype
described in §4. Every task in this plan must preserve unrelated work and
follow the task ordering in §15.

**Authority:** [language chapter 06](../language/06-machine.md) remains the
normative machine contract. This document owns implementation layering, host
dependency policy, backend conformance, remote execution, measurement
methodology, and task ordering. The
[Pixels implementation plan](WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md)
continues to own the display ABI and tasks P8.8a–P8.8e. If this plan and the
machine chapter disagree about guest-visible behavior, fix the machine chapter
first and reconcile this plan in the same commit.

**Sequencing (decided 2026-08-15):** This program executes in full, Tasks
V0–V15, between the close of the Pixels P8R tightening milestone and Pixels
Task P9.1. Task V0 serializes behind the P8R close because both edit chapter
06 and the Pixels plan. Tasks V13–V15 own hardware validation of the renderer
cycle proxy and the `a76-pi5` cost profile; Pixels Task P13.2 consumes their
standing drift lock before sealing the exact proxy.

---

## 0. Decision

Wrela has **one machine and two host backends**:

- macOS/Hypervisor.framework (HVF) is the native development host;
- Linux/KVM on Raspberry Pi 5 is the flagship appliance host and the
  authoritative physical performance target.

The compiler, image format, memory map, scheduler semantics, devices,
record/replay boundary, and guest code are shared. HVF and KVM do not implement
two Wrela platforms. Each is a thin adapter from one host hypervisor API to the
same Wrela-owned VMM engine.

The sealed image also owns the common guest translation description. The
compiler emits deterministic identity-mapped stage-1 tables and the boot
description used by both backends. DRAM is Normal write-back memory, machine
MMIO is Device memory, text is RX, writable data is NX, and `SCTLR_EL1.WXN` is
enabled. Host stage-2 protections remain defense in depth; they do not define
different guest machines.

The sole physical target is the 1 GiB board documented in the
[Rasputin target profile](rasputin-target-profile.md): four physical A76
cores, three guest vCPUs, one housekeeping core, and a 512 MiB guest DRAM
reservation. Larger Pi variants are not alternate product profiles.

```text
source / Wrela Forge
        |
        v
Wrela compiler on macOS
        |
        | sealed image + image report
        v
portable Wrela VMM engine
        |
        +----------------------------+
        |                            |
        v                            v
macOS host adapter              Linux host adapter
HVF + Metal/headless            KVM + DRM/headless
        |                            |
        v                            v
daily development               Rasputin / product Pi
functional iteration            target measurements
```

The cross-host abstraction is defined by the operations Wrela needs. It is not
a generic hypervisor framework and it does not expose the union of KVM and HVF
features.

The Linux backend uses the rust-vmm `kvm-ioctls` and `kvm-bindings` crates.
Wrela continues to own the abstraction above them. The existing direct HVF FFI
is retained initially. `applevisor`, `vm-memory`, generic rust-vmm devices, and
whole third-party VMMs are not adopted by this plan.

Rasputin is a remote execution and measurement target, not the Wrela
development environment. Source editing, compiler work, image construction,
golden review, and result analysis stay on the Mac. The Pi receives a cached
cross-built VMM binary plus sealed image artifacts and returns stable result
records. Building on the Pi is a documented fallback, not the primary lane or
release provenance.

## 1. Why this split exists

The Mac and Pi solve different problems:

| concern | authority |
|---|---|
| compiler iteration, source tooling, diagnostics | Mac |
| functional guest execution | both |
| deterministic machine behavior | backend conformance on both |
| frame bytes and record/replay | shared VMM engine, checked on both |
| Wrela Forge interaction and graphical iteration | Mac/HVF |
| Cortex-A76 scheduling and static proxy ranking | compiler profile |
| physical wall time, cache behavior, contention, thermals | Pi/KVM |
| DRM/KMS presentation and appliance integration | Pi/KVM |

Apple Silicon executes the same AArch64 baseline quickly and is the correct
daily host. It is not a Cortex-A76/BCM2712 performance oracle. Conversely, the
Pi is the correct product target but is a poor place to run the entire editor,
compiler, corpus, and graphics-development workflow.

The design therefore separates three evidence lanes:

1. **Static target evidence:** compiler dumps and the `a76-pi5` cost profile,
   produced on the Mac and independent of host wall time.
2. **Functional execution evidence:** boot goldens, record/replay, display
   digests, and backend conformance, normally exercised through HVF and
   repeated through KVM.
3. **Physical target evidence:** explicit Rasputin runs for wall time,
   counters, temperature, throttling, affinity, and sustained presentation.

No result is promoted from one lane into another. In particular, an M-series
wall time is not relabeled as Pi performance, and a physical timing result does
not replace the compiler's deterministic cycle-proxy verdict.

Validation is not promotion. Lane-3 evidence never enters compiler admission
or `cargo xtask verify`, but the cycle proxy's authority must never exceed its
validated accuracy: every sealed proxy revision is checked against flagship
hardware for one-sided conservatism, bounded overprediction, and rank
fidelity (§13.4, Tasks V13–V15). A validation failure is a model defect closed
through published-record provenance or state-model extension — never by
fitting an observed number.

## 2. Goals

This plan delivers:

1. one portable VMM engine containing all Wrela machine behavior;
2. a narrow, statically selected HVF/KVM host API;
3. an encapsulated guest-memory type shared by vCPUs and device models;
4. normalized exits with backend-private completion mechanics;
5. identical image, device, scheduler, and replay semantics on both hosts;
6. single-core and multicore Linux/KVM boot on Raspberry Pi 5;
7. headless cross-host conformance over the existing boot corpus;
8. a deterministic remote runner for Rasputin;
9. stable capability, conformance, and benchmark evidence formats;
10. separate headless, Metal, and DRM presentation sinks;
11. a Mac-native execution path suitable for Wrela Forge;
12. a reviewed, narrow exception to the repository's Cargo dependency rule;
13. product-acceptance checks for memory protection, resource reservation,
    host isolation, and packaging;
14. one compiler-emitted stage-1 translation and W^X contract shared by HVF
    and KVM;
15. a Mac-side Linux/aarch64 cross-build in the required repository gate once
    KVM code lands;
16. a sealed cycle-proxy validation contract — one-sided conservatism, a
    bounded overprediction envelope, and rank fidelity — with a stable
    evidence format;
17. single-core kernel and multi-worker full-frame validation of the renderer
    cycle proxy and `a76-pi5` cost profile under the accepted product host
    configuration;
18. a standing drift lock binding every sealed proxy revision to current,
    provenance-complete hardware-validation evidence.

## 3. Non-goals

This plan does not:

- create host-specific Wrela machine revisions or memory maps; if the common
  stage-1 contract cannot amend draft machine v1 safely, it bumps the one
  shared machine revision before implementation;
- add runtime platform discovery to Wrela programs;
- allow a guest to select HVF, KVM, core count, or device topology;
- emulate Raspberry Pi peripherals inside the guest;
- add UEFI, firmware, a device tree, PSCI, or an emulated GIC;
- adopt QEMU, Firecracker, Cloud Hypervisor, crosvm, Uhyve, or libkrun as the
  product VMM;
- adopt a general cross-platform virtualization framework;
- replace Wrela's closed device models with generic rust-vmm devices;
- make SSH or a live Pi part of `cargo xtask verify`;
- make network latency part of a benchmark;
- treat a Mac benchmark as a product performance result;
- make the Pi an interactive source-development environment;
- implement the Wrela Forge editor UI;
- add hot reload or mutate a running sealed image;
- render Wrela pixels on the host;
- permit a host presenter to repair, reinterpret, or partially present a
  failed frame;
- make a physical measurement a conformance or admission input; hardware
  evidence validates proxy revisions (§13.4) and physical regression locks,
  never `cargo xtask verify` or compiler admission;
- fit, tune, or infer a cycle-proxy transition constant from an observed
  sample; validation falsifies, published-record provenance defines.

## 4. Current repository state

The design is already normative in prose but not yet reflected in the VMM
layering:

- `docs/language/README.md` and chapter 06 name Linux/KVM and
  macOS/Hypervisor.framework as the two product hosts.
- `crates/wrela-machine` already owns the fixed memory map, report types,
  display records, pending-vector words, and machine revision.
- `crates/wrela-vmm/src/hv.rs` is a small direct HVF FFI and ARM exit decoder.
- `crates/wrela-vmm/src/boot.rs` validates images, allocates DRAM, creates the
  VM/vCPUs, initializes registers, schedules cores, handles exits, services
  devices, records choices, and assembles `BootOutcome` in one macOS-gated
  implementation.
- The compiler emits no `MSR` instruction for translation control, and the VMM
  initializes no `TTBR0_EL1`, `TCR_EL1`, `MAIR_EL1`, or `SCTLR_EL1`; every
  current guest therefore runs MMU-off for its entire life.
- Current HVF W^X is stage-2-only: DRAM begins read/write and executable report
  sections are changed to read/execute at the 16 KiB protection granule.
- `crates/wrela-vmm/src/exit_loop.rs` is mostly portable device/scheduler work,
  but PC and EL1 diagnostic helpers call HVF directly.
- Host vector raises and block interrupt status currently use non-atomic
  read/OR/write sequences while the guest clears pending words with ordinary
  loads and stores; V1 treats this as a correctness race, not merely new KVM
  work.
- `crates/wrela-vmm/src/devices.rs` has a Wrela-owned checked guest-memory view
  for virtio-blk.
- `crates/wrela-vmm/src/display.rs` already validates and digests guest-owned
  display records without rendering on the host. It is the beginning of the
  correct portable display boundary.
- `crates/wrela-vmm/src/boot_kvm.rs` is a direct-UAPI KVM prototype boot path
  with manually defined ioctl encodings and hardcoded `kvm_run` field
  offsets, routed from `boot.rs` on Linux/aarch64. It performs no capability
  validation and is nonconforming with §6 and §10: its manual UAPI layouts
  are exactly what the rust-vmm exception exists to avoid. Task V0
  inventories its observed behaviors and tests; Task V4 replaces and deletes
  it.
- `crates/wrela-vmm/src/display/kvm.rs` is a prototype DRM presenter; the
  §9.2 presentation split rebuilds it as `display/drm.rs` under Pixels
  P8.8e.
- `cargo xtask verify` runs a Linux/aarch64 `cargo check` lane for
  `wrela-vmm`; this plan considers `cargo check` insufficient, and Task V4
  upgrades it to a full build.
- `crates/wrela-vmm/src/main.rs` accepts an image report and blob plus record,
  replay, and Lane 2 output options. Its process contract is already suitable
  for a remote runner.
- `crates/xtask` signs and runs the required HVF lane, provides a Mac guest
  benchmark, and has no remote-host commands.
- `cargo xtask verify` requires macOS/aarch64 and is the only repository gate.

The first implementation work is an extraction that preserves all current
HVF behavior. KVM code must not be added by copying the whole boot loop and
editing it until it works. The existing KVM prototype is disposable evidence:
its observed behaviors and tests are inventoried at V0 and survive as named
fixtures where they matter, but the portable engine is not derived from it,
and V4 replaces it wholesale rather than rehabilitating its manual UAPI.

## 5. Ownership and layering

### 5.1 Crate ownership

`wrela-machine` owns guest-visible facts:

- machine revision and ISA baseline;
- guest physical addresses and sizes;
- report schema;
- device record encodings;
- queue, display, and pending-vector contracts;
- constants shared by compiler, runtime, and VMM.

`wrela-vmm` owns host implementation:

- report and image validation;
- guest-memory allocation and checked host access;
- portable scheduler and machine state;
- device models;
- record/replay;
- normalized exit handling;
- HVF and KVM adapters;
- headless and interactive presenters;
- process exit behavior.

`xtask` owns repository and lab orchestration:

- signed HVF verification;
- backend-conformance orchestration;
- Rasputin probing, deployment, running, and measurement;
- stable result parsing and comparison;
- no guest-visible semantics.

Wrela Forge is a client of the compiler and VMM process contracts. It does not
own a compiler fork, renderer, device model, or VM implementation.

### 5.2 Proposed source layout

The exact file split may follow the code as long as ownership stays singular.
The intended shape is:

```text
crates/wrela-vmm/src/
    lib.rs
    main.rs
    boot.rs                 image loading and top-level orchestration
    engine.rs               portable VM state and exit dispatch
    scheduler.rs            portable core scheduling and park/wake policy
    guest_memory.rs         allocation, bounds, volatile access, ownership
    host/
        mod.rs              the only portable host API
        hvf.rs              Hypervisor.framework implementation
        kvm.rs              Linux/KVM implementation
    devices.rs              portable Wrela device models
    display/
        mod.rs              portable display validation and sink contract
        headless.rs         digest/replay sink
        metal.rs            macOS presentation; Pixels P8.8d owner
        drm.rs              Linux presentation; Pixels P8.8e owner
    record.rs
    replay.rs
    lane3.rs

crates/xtask/src/
    pi.rs                   probe/build/deploy/run/bench orchestration
    backend_conformance.rs  stable cross-host comparison
```

Do not split files merely to match this tree. Split when a module acquires one
of the owners above. In particular, a generic `platform.rs` containing a mix of
KVM, Metal, SSH, and scheduler code is not an acceptable interpretation.

## 6. The portable host boundary

### 6.1 Shape

The portable engine compiles against one statically selected `host` module.
Dynamic dispatch is unnecessary: one VMM process has exactly one host
hypervisor. The HVF and KVM modules expose the same small concrete API.

The names below are illustrative; their responsibilities are fixed:

```rust
pub(crate) struct Vm;
pub(crate) struct Vcpu;
pub(crate) struct VcpuHandle;

pub(crate) struct HostCapabilities {
    pub backend: BackendKind,
    pub page_size: usize,
    pub max_vcpus: usize,
    pub ipa_bits: u8,
    pub isa: IsaCapabilities,
}

pub(crate) struct VcpuBootState {
    pub x0: u64,
    pub pc: u64,
    pub pstate: u64,
    pub sp_el1: u64,
    pub cpacr_el1: u64,
    pub stage1: Stage1BootState,
}

pub(crate) enum Stage1BootState {
    DisabledDiagnostic,
    Enabled {
        ttbr0_el1: u64,
        tcr_el1: u64,
        mair_el1: u64,
        sctlr_el1: u64,
        vbar_el1: u64,
    },
}

pub(crate) enum HostExit {
    Mmio(MmioAccess),
    Breakpoint { immediate: Option<u16> },
    Canceled,
    Unexpected(HostExitDiagnostic),
}

pub(crate) struct MmioAccess {
    pub address: u64,
    pub width: u8,
    pub direction: MmioDirection,
    pub write_value: Option<u64>,
}

pub(crate) enum MmioCompletion {
    WriteAccepted,
    ReadValue(u64),
}
```

`DisabledDiagnostic` exists only to preserve the pre-V5 HVF baseline and run
the V4/V4a measurement fixture. After V5, ordinary sealed-image execution
requires `Enabled`; selecting the diagnostic state is an explicit test/lab
option and is never product-conforming.

Required operations are limited to:

- query and validate host capabilities;
- create and destroy one VM;
- map the Wrela DRAM reservation;
- establish the accepted memory protections;
- create one vCPU on its owning thread;
- initialize Wrela's fixed register state;
- run and normalize one vCPU exit;
- complete an MMIO access using backend-private mechanics;
- read the fixed diagnostic register set after a guest fault;
- obtain a cross-thread handle that can kick/cancel a running vCPU;
- destroy resources in an order that cannot leave a live vCPU referring to
  unmapped memory.

There is no public generic get/set-register API in the portable engine. The
backend receives `VcpuBootState`, `MmioCompletion`, and a fixed diagnostic
request. This prevents KVM or HVF register identifiers from leaking upward.

The two host modules implement one internal `HostBackend` trait consumed as
`Engine<H>` with static dispatch. This adds no vtable, mechanically checks
both adapters against one interface, and admits a synthetic fake backend for
portable exit-normalization tests. An MMIO exit carries a completion token
consumed exactly once; a dropped or doubled completion is a fail-closed
error, never a silent skip.

`VcpuBootState` is illustrative shorthand, not the whole contract. Task V2
publishes a complete boot/visibility table covering every architecturally
observable initial value: x0–x30, SP_EL0/SP_EL1, PSTATE/DAIF, FPCR/FPSR and
the SIMD file, MPIDR/MIDR and every readable ID register, counter/timer and
PMU accessibility, cache-identification registers with permitted
cache-maintenance and `DC ZVA` behavior, and the disabled SVE, pointer
authentication, MTE, PSCI/hypercall, and debug surfaces. A value a backend
cannot pin must be proven unobservable by generated code through a sealed
image certificate carrying the complete system-instruction allowlist; "the
compiler does not emit it" is not evidence without that check. On KVM, the
guest IPA width is selected at `KVM_CREATE_VM` through the machine type, and
writable ID-register masking follows the kernel's documented ordering before
any other vCPU register access. On HVF, the required minimum macOS version
and any configurable IPA/granule APIs are recorded as host capabilities.

The published table and executable-section certificate are
[vmm-host-boot-visibility-v1](vmm-host-boot-visibility-v1.md). That document is
normative for the internal host boundary; the machine chapter remains
authoritative for guest-visible semantics.

### 6.2 Exit normalization

HVF and KVM report MMIO differently:

- HVF reports an ARM exception. The adapter decodes ESR, reads or writes the
  named GPR, and advances PC when the portable engine accepts the access.
- KVM reports `KVM_EXIT_MMIO`. The adapter copies write data out and copies
  read data back into the `kvm_run` mapping before the next `KVM_RUN`.

The portable engine sees the same `MmioAccess` and returns the same
`MmioCompletion`. It never advances a PC, edits a `kvm_run` union, or decodes a
host-specific result code.

The backend must fail closed when it cannot normalize an exit exactly. It may
not guess a register, access width, sign extension, instruction length,
physical address, or completion behavior.

Breakpoint, cancellation, and unexpected-fault exits follow the same rule.
The Wrela machine has no virtual timer device, so an HVF virtual-timer reason
or a KVM timer event is normalized to `Unexpected`, not elevated into dead
portable surface. Backend diagnostics retain raw values for error reporting,
but raw host values do not enter record/replay or guest-visible behavior.

### 6.3 Threading and lifecycle

Each vCPU is created and run on its owning host thread. A separate
`VcpuHandle` contains only the state needed for a permitted asynchronous exit.
It cannot run the vCPU or mutate registers.

The lifetime order is:

```text
guest memory allocated
    -> VM created and memory registered
        -> vCPU threads create vCPUs
            -> vCPUs run and stop
        -> every vCPU is destroyed
    -> VM mappings and VM are destroyed
-> guest memory freed
```

RAII guards enforce that order on success, guest failure, host error, replay
divergence, and watchdog timeout. No backend handle is stored as an integer in
portable scheduler state. The scheduler stores `VcpuHandle` values.

### 6.4 Error model

Replace HVF-shaped portable errors with a backend-neutral host error:

```rust
VmmError::Host {
    backend: BackendKind,
    operation: &'static str,
    code: i64,
    detail: String,
}
```

Guest faults, malformed reports, bad images, replay divergence, and timeouts
remain backend-independent. A KVM error is never formatted as an HVF error,
and a backend error is never downgraded to a guest exit code.

Stable tests assert semantic fragments and structured fields rather than
pinning an operating system's prose for `errno` or `hv_return_t`.

## 7. Guest memory and protection

### 7.1 One owned reservation

`GuestMemory` owns exactly the fixed Wrela DRAM reservation and provides:

- checked GPA-to-host-offset conversion;
- initialization writes before vCPUs run;
- volatile scalar reads and writes while vCPUs may be running;
- aligned atomic u32/u64 load, swap, and fetch-OR operations with an explicit
  Rust ordering at every call site;
- ownership-token-gated bulk copies after the guest has published and
  transferred a complete region;
- explicitly scoped immutable slices only when no guest writer can race;
- device-pool reachability checks;
- alignment and host-page-size checks;
- a stable pointer for the lifetime of every VM mapping and vCPU;
- zeroing before first guest execution;
- no resize, remap, balloon, or overcommit after launch.

Device-specific ownership rules remain in their device models. `GuestMemory`
proves bounds and host-memory validity; it does not make every concurrently
written byte safe to borrow as a Rust slice.

There is no generic "bulk volatile slice" escape hatch. Rust has no portable
bulk-volatile operation, and a byte-at-a-time volatile loop is not an accepted
production framebuffer path. A bulk copy is permitted only after the device
model proves exclusive host ownership following a release publish/acquire
consume boundary. Without that proof, the implementation uses checked scalar
or word-sized volatile operations and measures their cost.

The existing `devices::GuestMem` pool-window validation is preserved and
rebased onto `GuestMemory`. It is not replaced with unchecked indexing.

### 7.2 Alignment and reservation

The host backend reports its mapping granule. Allocation alignment is at least
the greater of that granule, the fixed guest stage-1 granule, and Wrela's
largest published region alignment. The VMM refuses an unsupported granule or
an overflowing rounded size.

Executable sections, stage-1 table pages, and writable data each begin and end
on the common protection granule required by both backends. No executable byte
may share a host-protection page with writable data. The compiler proves this
layout invariant and dumps the padding and page ownership; the VMM validates
it again before mapping.

Development mappings may use anonymous populated memory. The product Pi lane
must prove that the full reservation is committed before guest entry and
cannot disappear under host overcommit. The accepted mechanism—preallocated
huge pages, locked populated pages, or an equally explicit reservation—is
recorded in the host profile record and product package. A lazy anonymous
mapping that can fail halfway through a frame is not product-conforming.

### 7.3 Stage-1 translation and W^X are one common machine contract

The current guest runs for its entire life with stage-1 translation disabled:
the compiler emits no `MSR` sequence for `TTBR0_EL1`, `TCR_EL1`, `MAIR_EL1`, or
`SCTLR_EL1`, and the VMM does not initialize those registers. This is both a
protection gap and a physical-KVM performance risk. With stage 1 disabled,
ordinary data accesses receive the architecture's restrictive default memory
attributes unless EL2 can override them. The magnitude on Raspberry Pi is an
empirical question; the risk is not deferred until the final benchmark task.

The protection threat model is explicit: stage-1 W^X defends a trusted,
sealed, compiler-generated guest against its own defects — a stray store
cannot silently rewrite text, and data cannot become executable. It is not
hostile-guest isolation: EL1 owns `SCTLR_EL1` and its own tables, so a
malicious guest could undo stage 1. The enforcement consequence is
compiler-owned: generated code contains no system-register writes outside the
sealed boot sequence, checked by the same image certificate that owns the
system-instruction allowlist. Host stage-2 protection remains the
defense-in-depth floor against escaped writes to text. A hostile-image threat
model, if ever needed, is a separate design with stage-2 as the primary
boundary; this plan does not claim it.

The selected closure is compiler-emitted, sealed, identity-mapped stage-1
tables shared by HVF and KVM. Before Task V5 implementation, chapter 06 and the
stable image report define and dump:

- the fixed guest translation granule and reserved table range;
- deterministic table bytes and their image/report digests;
- exact `TTBR0_EL1`, `TCR_EL1`, `MAIR_EL1`, `SCTLR_EL1`, and `VBAR_EL1` entry
  values, including `SCTLR_EL1.WXN`;
- Normal, inner-shareable, write-back mappings for guest DRAM;
- Device mappings for every Wrela MMIO range so an access reaches stage 2 and
  exits to the VMM rather than becoming an unhandled stage-1 fault;
- RX mappings for executable image pages and RW/NX mappings for writable data;
- read-only mappings for the page-table pages themselves after entry;
- a complete permission-class matrix covering every guest physical region —
  text, rodata, frame programs, writable data, stacks, queue and device
  pages, framebuffer generations, table pages, guard pages, and unused
  gaps — with no byte left in an undeclared class;
- the exact diagnostic path for a stage-1 permission fault. The default
  candidate replaces the unmapped-vector two-fault path with a minimal fixed
  exception trampoline: a mapped RX vector page, a fixed RW/NX fault record
  capturing ESR/FAR/ELR/SPSR, and a fixed fault doorbell, normalized
  identically on HVF and KVM; V4a either adopts it or records why the
  two-fault path stays normative.

The compiler owns table construction, section separation, and boot values. The
VMM validates the report and mechanically installs the compiler-owned boot
state in the backend; it does not synthesize different tables or attributes
per host. If draft machine v1 cannot absorb the new reserved range and boot
state, the machine revision is bumped before code lands. That is expected
contract work, not a late stop condition. A revision bump must settle chapter
06's older-revision loadability promise in the same amendment: either MMU-off
machine-v1 images remain loadable under an explicitly diagnostic profile, or
the promise is amended before the bump lands.

Host stage-2 permissions remain defense in depth. HVF continues to protect
text pages read/execute. KVM probes `KVM_CAP_READONLY_MEM` and, when available,
registers text in read-only memory slots as the early write-protection floor.
That capability is recorded and checked rather than inferred from a kernel
version, and it does not replace stage-1 NX protection for writable data.

Task V4 may boot one explicitly diagnostic MMU-off image. Immediately after
that boot, Task V4a runs a small fixed loop and memory-copy workload in paired
MMU-off and provisional MMU-on configurations on Rasputin. It records raw
samples, exit counts, kernel/capability provenance, and identical output
digests. No predicted multiplier appears in the contract. Task V5 then lands
the production compiler-emitted tables on both hosts before multicore,
devices, replay parity, or product performance work proceeds.

## 8. Host backends

### 8.1 HVF

The first backend task moves the current behavior without changing it:

- direct link to Hypervisor.framework;
- one VM per process;
- 16 KiB-compatible mapping behavior generalized through `GuestMemory`;
- one vCPU per owning thread;
- ESR-based MMIO and breakpoint decoding;
- `hv_vcpus_exit` watchdog and wake behavior;
- existing EL1 diagnostic note;
- current signed test coverage and test census;
- current record/replay and boot transcripts.

The direct FFI remains private to `host/hvf.rs`. `applevisor` is not added
during extraction. A later replacement must be a dedicated behavior-preserving
task with before/after unsafe-code inventory, lifecycle tests, binary-size
change, and exact signed HVF coverage. Convenience alone is insufficient.

### 8.2 KVM

The KVM adapter is deliberately small:

1. open `/dev/kvm` and require the supported API version;
2. query every capability Wrela relies on;
3. create an arm64 VM without an in-kernel GIC;
4. register the fixed guest-memory regions;
5. obtain and validate the preferred arm64 vCPU target;
6. create and initialize exactly the report-declared vCPUs;
7. validate the host has every Wrela baseline ISA feature before any
   `KVM_RUN`, and mask discoverable feature registers where KVM supports doing
   so;
8. set the Wrela boot registers through the arm64 one-register API;
9. normalize `KVM_EXIT_MMIO`, cancellation, system, debug, and internal-error
   exits;
10. kick a running vCPU using a capability-checked immediate-exit or signal
    path;
11. return structured host errors for every unsupported or failed operation.

Stage 1 identity-maps Wrela MMIO virtual addresses to Device-typed IPAs. Those
IPAs have no KVM memory slot, so stage 2 returns accesses to userspace. The
backend does not create a VGIC, PIT, serial port, PCI bus, firmware region, or
kernel boot protocol.

KVM's preferred target and feature IDs are host inputs to a validation step,
not guest discovery. The enforceable invariant is compiler-owned: generated
code uses only baseline instructions and does not read feature ID registers.
Both hosts refuse hardware missing the baseline. KVM feature masking is
defense in depth because HVF cannot promise an equivalent mask. Future
activation of `FEAT_DotProd` follows the existing Pixels P12 machine amendment
and must update the compiler and both host validators together.

### 8.3 Interrupts, wakes, and parks

There is no virtual interrupt controller on either host. A Wrela vector raise
is still:

1. an aligned atomic `fetch_or(Release)` into the target core's pending word;
2. a wake/kick if that vCPU is parked or running outside the shared scheduler;
3. an acquire atomic load at the guest's checkpoint fast path followed by an
   atomic `swap(0, Acquire)` to claim the pending bits;
4. observation by the guest only at that checkpoint or park boundary.

A plain store or read/OR/write is forbidden because it can clobber a different
concurrent vector. The same rule applies to host-injected device status words:
the host ORs status atomically, the guest observes it with acquire semantics,
and acknowledgement cannot race a simultaneous raise into losing a bit. The
pending and status words have compile-time and runtime alignment checks, and no
non-atomic host or guest access aliases them while vCPUs can run.

The wake mechanism is backend-specific; the injection point is not. KVM must
not inject an IRQ to approximate the HVF path.

Park deadlines, watchdog behavior, replay choices, and core baton scheduling
stay in the portable engine. A host wake may cause an extra raw exit but may
not add a record/replay choice or change which checkpoint consumes a vector.

## 9. Devices and presentation

### 9.1 Device models are portable

Clock, entropy, console, block, display, admission ordering, and replay remain
Wrela-owned portable models. A rust-vmm virtio implementation is not substituted
for the machine-v1 protocol. This preserves the closed feature set, validation
messages, queue ceilings, ownership checks, and golden behavior.

The host backend knows only that an MMIO access occurred. It does not know what
`CLOCK_MMIO_ADDR`, `PARK_MMIO_ADDR`, a block doorbell, or a display control
record means.

### 9.2 Display validation and presentation are separate

The portable display device:

- acquires a published frame generation after the guest's release boundary;
- reads control, tile, and pixel records only while that generation is
  host-owned;
- validates every address, count, format, sequence, and ownership rule;
- assembles the complete BGRA frame;
- computes and records the canonical digest;
- emits a complete immutable presented-frame value;
- never modifies a guest pixel.

A presenter consumes only that validated complete frame:

```rust
trait Presenter {
    fn present(&mut self, frame: &PresentedFrame) -> Result<(), PresentError>;
}
```

The actual signature may avoid a public trait, but the ownership boundary is
fixed. Headless, Metal, and DRM sinks are downstream of validation and digest
creation. A presenter failure is a host error; it does not mutate the digest,
pretend that a frame was shown, or expose a partial frame.

The display implementation consumes a published region through
`GuestMemory`'s ownership-gated bulk-copy primitive. It does not retain the
current per-byte volatile copy as the production frame path, and it does not
construct a shared Rust slice over guest-writable memory. Completion releases
the prior generation back to the guest only after validation, copying, and
digest creation finish.

The Linux product presenter is KMS dumb-buffer scanout: CPU-written frames
with no Mesa/V3D or GPU driver in the presentation TCB. A GPU-composited
presenter, if ever wanted, is a separate design with its own dependency
amendment. `present()` semantics are pinned per sink — for DRM, a frame is
"presented" when the page flip for its buffer is queued and acknowledged, and
vblank completion is reported distinctly; a sink may not report a frame
presented while its bytes are still being copied.

Pixels tasks P8.8a–P8.8c own the portable device and headless replay. P8.8d
owns Metal presentation. P8.8e owns Linux/DRM presentation. This plan supplies
the host and presenter seams those tasks consume; it does not create duplicate
task histories.

### 9.3 Input follows the same split

Mac window events and Linux input events are host presentation/input concerns.
They are translated into the one Wrela input queue before entering the
portable device model. Host key codes, event timestamps, controller IDs, and
repeat behavior do not enter the guest ABI directly.

The implemented file transport opens its source once, retains that descriptor,
and rejects path replacement. Each poll consumes only the file extent captured
at the start of that poll, so a concurrent append is deferred intact to the
next poll; shutdown rejects a trailing unterminated event rather than silently
dropping it.

## 10. Dependency policy

### 10.1 Approved exception

At the design basis, the repository says `No new dependencies`. Implementing
KVM by manually copying Linux UAPI structures and ioctl encodings would move a
large, architecture-sensitive unsafe ABI into Wrela without adding product
value. Chapter 06 already permits the VMM to build on rust-vmm crates; this
plan supplies the exact dependency and review boundary for that permission.
It therefore approves one narrow policy amendment:

> No new dependencies except reviewed, target-gated host ABI bindings named in
> an approved design. Wrela owns the machine, scheduler, devices,
> record/replay, presentation contract, and cross-host abstraction.

Before adding Cargo entries, Task V0 updates `AGENTS.md` with the exact
allowlist and review conditions. Until that task lands, the current rule still
applies.

The initial allowlist is Linux/aarch64-only:

```toml
[target.'cfg(all(target_os = "linux", target_arch = "aarch64"))'.dependencies]
kvm-ioctls = "=0.25.0"
kvm-bindings = "=0.14.1"
```

These were the current releases at the design date. Task V0 rechecks the
[upstream rust-vmm `kvm` workspace](https://github.com/rust-vmm/kvm),
Rust-version compatibility, Raspberry Pi kernel compatibility, licenses,
source integrity, and complete locked transitive closure before pinning. A
version change is recorded as a design deviation with the reason; floating
ranges are not accepted.

The expected transitive runtime closure includes `libc`, `bitflags`, and
`vmm-sys-util`. The lockfile, not a prose count, is authoritative. Default or
optional features not required by Wrela are disabled where possible.

### 10.2 Why these crates qualify

`kvm-bindings` owns generated architecture-specific KVM layouts.
`kvm-ioctls` owns safe file-descriptor and ioctl wrappers, `kvm_run` mapping,
capability calls, arm64 vCPU initialization, and exit access. These are kernel
ABI obligations with many non-obvious layout and lifetime rules.

Wrela wraps both crates immediately. No rust-vmm type appears in
`wrela-machine`, a stable dump, a record file, a device model public API, or a
Forge interface.

### 10.3 Explicitly declined dependencies

| candidate | decision | reason |
|---|---|---|
| [`applevisor`](https://github.com/impalabs/applevisor) | not now | current HVF FFI is small; wrapper memory/protection ownership must prove a net reduction |
| `applevisor-sys` | not now | generated surface is much larger than the fixed calls Wrela uses |
| `vm-memory` | defer | one fixed reservation and ownership-sensitive volatile access are already narrower |
| rust-vmm virtio crates | decline | Wrela has a closed, versioned device contract and validation behavior |
| Uhyve | decline | specialized executable/VMM, not Wrela's host ABI boundary |
| Firecracker/Cloud Hypervisor/crosvm | decline | implement different machines and substantially enlarge the TCB |
| QEMU/libkrun | bootstrap/reference only | wrong product boundary and device ownership |
| a generic KVM/HVF crate | decline | no mature implementation matches both arm64 hosts and Wrela's semantics |

Any later dependency needs its own design amendment. The KVM exception is not
precedent for adding logging, async, CLI, serialization, SSH, DRM, Metal, or
GUI crates casually.

## 11. Mac development and Wrela Forge

### 11.1 Daily loop

The ordinary development path is:

```text
edit Wrela source
    -> compile locally
    -> produce sealed image + report
    -> launch a fresh HVF VMM process
    -> interact through Wrela input devices
    -> present through headless or Metal sink
    -> retain transcript, record, frame digests, and diagnostics
```

Rebuild/reboot is the first iteration mechanism. Wrela images are sealed; the
development host does not rewrite code, replace a frame program, or inject new
objects into a running guest. Fast compiler and cold-boot performance make a
clean restart preferable to a second mutable runtime contract.

### 11.2 Forge boundary

The implemented v1 process contract is recorded in
[`forge-process-contract.md`](../forge-process-contract.md).

Forge orchestrates the real compiler and VMM. The initial integration may
launch `wrela-vmm` as a child window/process. A later embedded UI may add a
local versioned presentation/input transport carrying complete validated
frames and Wrela input events.

That transport is development-host IPC, not a machine device. It includes
frame sequence and digest, does not carry an alternate scene representation,
and cannot bypass display validation. A Forge crash may terminate its VMM
child; it cannot corrupt a packaged image.

Forge may visualize compiler-owned field graphs, certificates, costs, and
other dumps. Such views are labeled tooling evidence. The authoritative image
shown as Wrela output is always produced by guest AArch64 code through the
display device.

A local unsigned development run may use an explicitly labeled diagnostic
host profile. It is not product-conforming. Chapter 06's macOS sandbox promise
is closed in the host-hardening task with the signed VMM/Forge child process,
filesystem/device allowlist, resource limits, and negative tests, or chapter
06 is amended before release. Linux hardening does not silently satisfy the
macOS requirement.

## 12. Rasputin remote execution

### 12.1 Command surface

The first interface is maintainer-only `xtask` orchestration:

```text
cargo xtask pi probe rasputin
cargo xtask pi prepare rasputin
cargo xtask pi run rasputin <golden-case-or-image>
cargo xtask pi conformance rasputin
cargo xtask pi bench rasputin <workload>
```

`rasputin` is an explicit SSH host argument, not a hard-coded production name.
Argument arrays protect only the local exec: OpenSSH concatenates remote
command arguments into one string that the remote shell re-interprets, so
quoting is never accepted as a security boundary. The runner therefore
executes exactly one constant literal remote command — the deployed
`wrela-lab-agent` helper — and passes the run manifest and every variable
value over stdin. Artifact transfer uses SFTP, not shell-composed copies. The
agent validates paths against the configured lab root, creates the run
directory, supervises the VMM under the remote deadline (so a local timeout
cannot orphan a remote process), and emits one canonical result stream. SSH
runs with forwarding, agent forwarding, and TTY allocation disabled and the
host key pinned. No Rust SSH dependency, shell interpolation, password
storage, or privilege escalation is added; the agent is cross-built, cached,
and deployed with the same content-addressed discipline as the VMM binary.

The runner refuses an empty host, option-shaped host, control characters,
unexpected remote path, malformed result, digest mismatch, or remote command
that succeeds without producing its declared artifact.

### 12.2 Probe

`pi probe` is read-only. It populates the split host records of §12.5 —
stable identity, declared profile, and per-run environment — and records at
least:

- architecture and kernel release;
- EEPROM second-stage bootloader version and configuration digest;
- KVM API version and required extension verdicts;
- `KVM_CAP_READONLY_MEM` and every other protection capability used;
- access to `/dev/kvm` without `sudo`;
- host page size and mapping granule;
- preferred arm64 target and maximum vCPU count;
- guest IPA width;
- Wrela baseline ISA feature verdict;
- total and available memory;
- configured huge-page or locked-memory reservation mechanism;
- CPU model and online-core set;
- current governor and frequency policy;
- thermal zones and throttling flags;
- DRM/KMS availability when presentation is requested;
- local cross-toolchain target and linker identity recorded with the deployed
  VMM;
- remote Rust toolchain identity only when fallback remote building is
  enabled.

Probe values are host evidence, not machine discovery passed to the guest. A
missing required capability produces `verdict=refused` with named reasons.
Task V0's dependency-free preflight fills only facts available from stable host
interfaces and records KVM ioctl fields as `not-queried`. Task V4 completes the
probe through the target-gated KVM backend. The preflight does not copy ioctl
numbers into xtask or infer a KVM capability from a kernel version string.

### 12.3 Build and deployment

The primary implementation cross-builds `wrela-vmm` on the Mac for
[`aarch64-unknown-linux-musl`](https://doc.rust-lang.org/rustc/platform-support/aarch64-unknown-linux-musl.html)
with the pinned Rust toolchain and linker. It then transfers the static binary
to Rasputin. Task V0 proves the target and linker preflight; Task V4 proves a
full link and execution. Once KVM source lands, `cargo xtask verify` performs a
full `cargo build` for the Linux/aarch64 target so target-gated source and
linker behavior cannot rot between physical runs. A `cargo check` alone is
insufficient.

The local binary cache key includes:

- source digest of the required manifests and crates;
- `Cargo.lock` digest;
- Rust compiler identity;
- target triple and profile;
- enabled features;
- dependency-source digest;
- product hardening mode.

Ordinary Wrela source iteration does not rebuild the VMM. It transfers only
the image report, image blob, optional replay record, and expected digests.

If musl, a transitive build script, or a kernel-facing dependency cannot be
linked or executed correctly, a content-addressed remote build of only the
required crates is the temporary focused-diagnosis fallback. The failure and
fallback toolchain identity are recorded; ambient remote Cargo state is not
release provenance. V4 does not close until a pinned Mac cross-target—musl by
default, or an explicitly reviewed GNU target/sysroot deviation—performs the
full build in `cargo xtask verify` and produces the deployed artifact. A
fallback bundle excludes target directories, goldens, documentation,
unrelated workspace sources where Cargo permits, and all credentials. Product
release still builds the minimal Linux host and VMM through a reproducible
pinned toolchain.

### 12.4 Run protocol

For each run, xtask:

1. creates a unique, narrow remote run directory;
2. transfers files under content-derived names;
3. verifies their SHA-256 values remotely;
4. runs the cached VMM with explicit paths and a wall cap;
5. writes transcript, stderr, record, host metadata, and result record into
   that directory;
6. retrieves and validates every declared artifact;
7. removes or retains the remote directory according to an explicit option;
8. reports the local artifact directory.

Network transfer and SSH setup occur outside the timed region. The timed loop
runs entirely on Rasputin and writes one result after all samples complete.

Remote cleanup targets only a validated per-run directory beneath a configured
Wrela lab root. No recursive command receives `~`, `/`, an environment-derived
empty string, a wildcard, or a workspace root.

### 12.5 Stable result formats

Six versioned, sorted, line-oriented formats are added. They use explicit
field encoding and existing hash utilities; they do not add a serialization
dependency. Task V0 seals the shared grammar — field ordering, delimiters,
escaping, list encoding, UTF-8 policy, maximum sizes, and unknown-field
rejection — as format-versioned facts rather than parser conveniences.

The host description is split so stable facts and per-run conditions never
share a digest:

`wrela-host-identity-v1` records stable facts and the acceptance verdict:
board and CPU identity, kernel release and configuration digest, KVM/DRM
module identity, EEPROM bootloader version and configuration digest, device
tree digest, PMU identity, and the KVM capability verdicts.

`wrela-host-profile-v1` records the declared product or diagnostic profile:
hardening mode, CPU isolation and affinities, the memory-reservation
mechanism, and expected governor and frequency policy.

`wrela-run-environment-v1` records observed per-run conditions: temperature,
throttle flags, available memory, online cores, actual frequencies,
IRQ/context-switch census, and display mode. It is fresh evidence for every
run and is never digest-bound as if stable.

Build provenance — source, lockfile, toolchain, linker, flags, and binary
digests — already rides in the binary cache key and is repeated in result
records.

`wrela-backend-conformance-v1` records, per case:

```text
case
machine_revision
image_sha256
report_sha256
backend
exit_class
guest_exit_code
transcript_sha256
record_sha256
choice_count
exit_count
frame_count
frame_digest_sequence
```

`wrela-pi-benchmark-v1` additionally records:

```text
host_identity_sha256
host_profile_sha256
run_environment_sha256
vmm_source_sha256
vmm_binary_sha256
stage1_tables_sha256
host_protection_profile
kernel_release
cpu_model
online_cores
vcpu_affinity
host_affinity
governor
frequency_policy
temperature_before_millic
temperature_after_millic
throttle_before
throttle_after
warmup_count
sample_count
samples_ns
min_ns
median_ns
max_ns
guest_exit_count
frame_count
frame_digest_sequence
optional_counter_rows
```

`host_identity_sha256` and `host_profile_sha256` name the exact canonical
identity and profile records accepted for the run; `run_environment_sha256`
names that run's observed conditions. Benchmark fields may repeat convenient
query columns, but disagreement with a referenced record is a parser error
rather than silently drifting provenance. `stage1_tables_sha256` must match
the sealed report. The protection-profile name must match the profile record
and distinguishes diagnostic, development, and product-conforming runs.

Counter rows name Linux perf events under canonical Wrela keys. The named
validation set is `cpu_cycles`, `inst_retired`, `l1d_cache_refill`,
`l2d_cache_refill`, `br_mis_pred`, `stall_frontend`, and `stall_backend`.
The rows remain optional for ordinary benchmarks; a §13.4 validation-class
run refuses to produce a report without the complete named set.

`wrela-proxy-validation-v1` records one validation run of a named proxy
revision against measured hardware evidence:

```text
proxy_revision
proxy_rules_sha256
cost_profile_sha256
corpus_manifest_sha256
holdout_manifest_sha256
host_identity_sha256
host_profile_sha256
run_environment_sha256
vmm_binary_sha256
counter_config            # raw event encodings, privilege filters, groups
measurement_error_model   # formula identity and parameters
per case:
  case
  workload_class          # kernel | frame | sequence
  corpus_set              # calibration | holdout
  image_sha256
  workload_sha256
  stage1_tables_sha256
  cache_state             # cold | warm
  predicted_cycles_per_core
  sample_count
  samples_cycles_per_vcpu
  measured_min_cycles
  measured_median_cycles
  measured_max_cycles
  measurement_error_cycles
  counter_rows
  conservatism_verdict
  overprediction_ratio_milli
per declared candidate pair:
  pair
  predicted_order
  measured_order
  discordant
summary:
  calibration_verdict
  holdout_verdict
  conservatism_violations
  max_overprediction_ratio_milli
  discordance_rate_milli
  verdict
```

A validation report is evidence about a proxy revision, never a conformance
input. Its parser applies the same fail-closed rules as the other formats and
additionally rejects a summary verdict not derivable from its rows. Content
digests prove integrity, not origin: operator, host, and retrieval provenance
fields record who produced a result, and reports enter `bench/results/` only
through review.

Parsers reject unknown versions, repeated fields, missing required fields,
unsorted case rows, noncanonical integers, inconsistent counts, digest length
errors, and a median not derivable from the sample list.

Physical capability and timing values are not golden-pinned as universal host
facts. The format/parser has synthetic goldens; named target results may be
checked in deliberately under `bench/results/` only when they carry full
provenance and a stated decision that consumes them.

## 13. Raspberry Pi measurement protocol

### 13.1 What is measured

The Pi lane separates:

- VMM creation and cold boot;
- guest workload duration;
- frame duration and sustained frame cadence;
- vCPU exit count and reason census;
- device service time where an explicit instrument exists;
- optional Linux perf counters;
- temperature and throttle state.

The first stable benchmark may retain whole-process boot time to compare with
the existing Mac guest benchmark, but render and steady-state product claims
need an in-guest or VMM-delimited interval that excludes SSH, file transfer,
remote process startup, and result retrieval.

Calibration and validation workloads delimit timed regions at existing
machine MMIO exits — the display doorbell and completion already bound a
frame, and kernel-family fixtures are whole-run delimited between entry and
guest exit. No measurement marker, PMU device, or other guest-visible surface
is added for measurement. PMU counters are read host-side on the pinned vCPU
thread.

### 13.2 Host control

A benchmark declares its required host state. The runner verifies rather than
silently changes:

- CPU governor and permitted frequencies;
- online cores;
- vCPU and VMM housekeeping affinity;
- no active throttling;
- temperature below the workload's starting ceiling;
- sufficient committed guest memory;
- no competing Wrela benchmark process;
- required perf-event permissions;
- expected kernel and VMM hardening profile.

Changing governor, frequency, online cores, limits, or privileges is an
explicit operator preparation action outside the benchmark. A diagnostic mode
may run on a mismatched host but its result is marked `nonconforming` and cannot
update a lock.

### 13.3 Sampling and locks

Each workload fixes warmup count, sample count, statistic, image digest, and
expected output digest before measurement. The runner prints every raw sample.
It does not retain only the median.

A physical threshold is added only after repeated conforming runs establish a
baseline and noise envelope. It is a regression lock, not a performance target.
Changing it follows the discipline in `bench/thresholds.toml`: a separate
review surface with old/new measurements and provenance. A failed lock is not
widened in the implementation commit that triggered it.

The Mac `cargo xtask bench guest` remains a Mac/HVF algorithmic regression
lock. Pi locks have distinct names and never share a numeric threshold with it.

### 13.4 Cycle-proxy validation

The compiler's renderer cycle proxy and the `a76-pi5` cost profile are
deterministic models. Their authority inside admission and optimization
ranking must never exceed their validated accuracy against the flagship
hardware; this lane is where that accuracy is established. Validation output
never enters compiler admission, `cargo xtask verify`, or a conformance
verdict. It gates whether a proxy revision may be sealed at all.

A validation run pairs, per corpus case, the proxy's predicted cycle total
with PMU-measured cycle counts from conforming Rasputin runs, keyed by proxy
revision, image digest, workload digest, and the accepted host identity and
profile records. Three properties are checked:

1. **One-sided conservatism (observed non-underprediction).** On the
   declared corpus under the declared conditions, no measured guest-cycle
   count exceeds the proxy's bound for the same case beyond the declared
   measurement-error term. This is corpus falsification evidence, not a
   universal physical proof; the corpus is therefore split into a
   calibration set and a frozen holdout set, and the holdout may not inform
   any model fix. A violation is a proxy-model defect and a stop condition;
   it is closed by a published-record fact or a state-model extension, never
   by fitting the observed number.
2. **Bounded overprediction.** The predicted-to-measured ratio stays inside a
   sealed per-workload-class envelope so proxy headroom is not fictitious. An
   envelope breach is a named finding that demands model-precision work; it
   does not change an admission verdict.
3. **Rank fidelity.** For declared candidate pairs drawn from real compiler
   selection decisions, the proxy's ordering agrees with the measured
   ordering wherever the measured difference exceeds the noise envelope.

The corpus must contain kernel-family fixtures, real renderer frames, and
minutes-long sequences. Microkernels alone are inadmissible evidence: ranked
decisions must survive real programs. Envelopes are sealed from repeated
conforming-run noise measurements, not chosen aspirationally, and every
report carries the full provenance of §12.5.

Proxy revision `a76-pi5-v1` sealed the frame, kernel, and sequence
overprediction envelopes at 3058, 2779, and 3110 milli-units respectively.
The release-atomic pending-vector correction changed executable placement
without changing the frame fixture's retired instruction count; repeated
conforming calibration runs therefore retired the current image below the V1
frame envelope. Proxy revision `a76-pi5-v2` retains the already-frozen corpus
and the kernel and sequence limits, and seals the fresh frame envelope at 3090
milli-units before opening that corpus's holdout. The revision is evidence of
the compiler lifecycle change, not permission to mutate a V1 limit in place.
A lifecycle rerun derives a fresh per-class measurement-error term from its
raw calibration samples but must not recompute, tighten, or widen these sealed
limits. Changing a limit requires a new proxy revision and a new frozen
holdout, never evidence from the current revision's holdout.

Measurement mechanics are part of the evidence, not an implementation
detail. Guest-cycle comparisons count guest execution only, using the arm64
perf `exclude_host`/`exclude_guest` attributes; every report records raw
event encodings, privilege filters, and pinned-group membership, and refuses
multiplexed samples (`time_enabled` must equal `time_running`). Counters are
collected per vCPU as vectors, and the comparison rule must match what the
proxy predicts: a per-core worst path is compared per core, never against a
sum. Overflow handling, PMU version, and cold/warm initial cache state are
declared per case. Guest-cycle evidence, wall-clock frame cadence,
presenter/vblank latency, and VMM/device service time are four separate
streams; a proxy over guest instructions is never compared against a sample
that includes DRM presentation or host service time.

Aggregate refill and mispredict counts are falsification diagnostics; they
cannot attribute events to particular proxy transitions. Attribution uses
controlled differential fixtures that vary one modeled term at a time — real
workloads falsify, fixtures attribute.

Validation is a reusable lifecycle, not a one-time event:

```text
draft proxy revision
    -> calibration runs (calibration set)
    -> model fixes via published-record provenance
    -> frozen holdout runs
    -> hardware validation report
    -> offline binding check in `cargo xtask verify`
    -> sealed proxy revision
```

Every later proxy revision — including Pixels P13.2's exact-proxy extension —
reruns this lifecycle; an earlier report never carries over to a new
revision. `cargo xtask verify` never runs physical measurement and measured
values never enter compiler admission, but it does verify the schema,
provenance, and digest binding of the checked-in validation report against
the active proxy revision. Without that offline check the drift lock is
policy rather than enforcement.

**Operator rerun policy.** Delete the checked envelope and report before
collecting a replacement; the runner then exposes only calibration cases.
Review and check in the derived envelope before unsealing the frozen holdout.
Run all three holdout classes, copy the canonical identity, profile, final
run-environment, envelope, and validation records from the runner's candidate
directory, and run `cargo xtask verify`. Never splice cases from different VMM
binaries, identities, or profiles. Rerun the entire calibration/holdout
lifecycle after any change to the proxy or cost rules, VMM source or build
toolchain, calibration/holdout manifests, kernel, EEPROM/boot configuration,
loaded KVM/DRM module identity, hardening profile, CPU topology/isolation, PMU
configuration, or display-presentation configuration. Temperature and
frequency observations remain per-run records; throttling invalidates the run.
If a rerun fails, retain its raw artifacts for diagnosis but leave the active
revision unsealed—do not widen an envelope in the correcting change.

## 14. Verification and conformance

### 14.1 Required repository gate

`cargo xtask verify` remains the one required task and milestone gate. It stays
local, deterministic, and macOS/aarch64. A live SSH host is never required for
the repository to verify. After V15, the local gate also performs the offline
validation-report binding check of §13.4 — schema, provenance, and digest
binding against the active proxy revision — without running any physical
measurement.

After Task V4, that local gate also performs a full pinned
`aarch64-unknown-linux-musl` build of `wrela-vmm`. This does not execute KVM or
replace Rasputin evidence, but it must compile and link every Linux/aarch64
backend path, its Linux-gated test modules, and its locked dependency closure.

The operator check for those Linux-only tests runs on a conforming Rasputin
checkout at the exact commit under review, as the unprivileged lab user with
`/dev/kvm` access:

```text
cargo test --release -p wrela-vmm --lib --features native-presentation -- --test-threads=1
```

Retain the command output with the commit, kernel identity, and host-profile
evidence. This is the execution lane for Linux ABI assertions and the spinning
KVM-guest watchdog regression; a successful Mac cross-build is not a substitute.
It remains an explicit physical-target check, not a second repository gate.

Portable engine logic, exit-normalization state machines, guest-memory bounds,
stable result parsers, and synthetic KVM exit fixtures run in the ordinary Mac
unit/golden lanes where possible. Signed HVF behavior remains in the required
gate.

Linux-only source cannot be considered validated merely because its portable
tests pass on macOS. Every KVM implementation task also names an explicit
Rasputin focused check. That focused check is required evidence for the task but
does not replace `cargo xtask verify` as the repository gate.

### 14.2 Backend conformance matrix

The same sealed image/report pair is run through both hosts. Comparison ignores
backend implementation details and wall time and requires equality where the
machine contract does:

| evidence | HVF | KVM | comparison |
|---|---:|---:|---|
| report/image digest acceptance | yes | yes | exact |
| guest exit code | yes | yes | exact |
| transcript | yes | yes | byte exact live for deterministic cases; exact under replay otherwise |
| replay choice sequence | yes | yes | exact under replay |
| device output digests | yes | yes | exact |
| frame digest sequence | yes | yes | exact |
| core marks | yes | yes | exact |
| named guest-fault class | yes | yes | exact semantic class |
| raw exit count | yes | yes | reported, not generally equal |
| host error text/code | yes | yes | backend-specific |
| wall time | yes | yes | never compared for conformance |

Live byte-exact rows use images whose outputs are deterministic without host
clock, entropy, or other recorded choices. For nondeterministic inputs, record
once and replay on both hosts before comparing transcript or device bytes. A
recording made on either backend must be parseable on the other because records
describe Wrela choices, not HVF or KVM exits.

The standing offline corpus checks entropy, monotonic time, and a real Pixels
frame case. It retains both canonical choice logs and the shared transcript,
retains all four backend/record cross-replay outputs, requires every replay to
reproduce the shared transcript exactly, and compares choice counts and frame
digest sequences while allowing live nondeterministic choice values and
backend-private raw exit counts to differ. Every record is bound to the KVM
identity, accepted product profile, host-binary digest, replay-matrix digest,
and a source contract covering the VMM, evidence grammar, conformance runner,
compiler proxy rules, and the exact fixture inputs. `cargo xtask verify`
rejects stale, missing, extra, or misbound checked evidence without contacting
Rasputin.

### 14.3 Test progression

KVM coverage lands in this order:

1. host capability refusal and API-version errors;
2. one vCPU, fixed register entry, console, and guest exit;
3. MMIO read and write widths plus hostile access failures;
4. paired MMU-off/MMU-on Rasputin microbenchmark evidence;
5. common compiler-emitted stage-1 boot and W^X negative cases on both hosts;
6. atomic pending/status races, park, deadline, wake, and watchdog
   cancellation;
7. multicore release, baton scheduling, and core marks;
8. entropy and clock record/replay;
9. virtio-blk success and hostile descriptor failures;
10. ownership-transferred display digest and replay;
11. cross-host corpus conformance;
12. committed-memory, Linux jail, macOS sandbox, and package acceptance;
13. Metal/DRM interactive presentation conformance.

Every discovered KVM failure becomes a permanent portable unit test, synthetic
exit test, or golden before its fix. A physical-only kernel behavior also keeps
a named Rasputin regression case.

## 15. Implementation program

Tasks are linear unless a task explicitly says it is coordinated with a Pixels
task. Each task gets one commit and runs `cargo xtask verify` before closing.
Focused checks are diagnostic evidence and do not substitute for the gate.
The program closes with the cycle-proxy validation tasks V13–V15; the full
V0–V15 sequence completes before Pixels Task P9.1 begins.

### Task V0 — reconcile machine, host, and dependency policy

**Requires:** this approved design, access to Rasputin, and no change to guest
behavior.

**Produces:** a read-only preflight host inventory; exact rust-vmm dependency
audit; updated `AGENTS.md` dependency exception; corrected machine and Pixels
documents; a pinned Mac cross-target/linker preflight; a labeled inventory of
the existing KVM/DRM prototype with the behaviors and tests that must survive
as named fixtures; and stable synthetic host identity, profile, and
run-environment schemas before KVM consumes them.

**Files:** `AGENTS.md`, this document, `docs/language/06-machine.md`,
`docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md`, cross-toolchain
configuration, `crates/xtask/src/pi.rs` (new), stable parser tests/fixtures,
and no production KVM code.

**Contract/dump delta:** adds `wrela-host-identity-v1`,
`wrela-host-profile-v1`, and `wrela-run-environment-v1`; no image, report,
machine, or record delta.

**Work:** preflight Rasputin's architecture, `/dev/kvm` access, page size,
memory, CPU identity, kernel, and DRM using stable host interfaces; mark
ioctl-only API, target, IPA, vCPU, read-only-memory, and feature rows
`not-queried`; prove that the pinned Linux/aarch64 Rust target and linker are
available locally; audit the exact Cargo closure and licenses; cite chapter
06's existing rust-vmm permission; update the repository rule with only the
approved target-gated crates and conditions; correct chapter 06's stale
`IMAGE_BASE`/`RTDATA_BASE`; add the normative common W^X requirement; correct
chapter 06's boot-chain TCB claim to name the Pi EEPROM second-stage
bootloader as versioned third-party boot firmware in the appliance path and
state how it is pinned and recorded; inventory the KVM/DRM prototype and name
the V4 removal boundary; seal the shared evidence-format grammar; and
reconcile Pixels P8.8's `display/hvf.rs`/`display/kvm.rs` names to the
presentation-owned `display/metal.rs`/`display/drm.rs`. Task V4 replaces
`not-queried` with capability-checked KVM values. Task V4a fixes the exact
stage-1 machine contract before implementation.

**Tests:** hostile parser cases; synthetic preflight, conforming, and refused
probe records; canonical `not-queried` fields; cross-target/linker preflight;
Pixels-plan path lint; machine-constant correspondence; argument validation
that cannot turn an SSH host or remote path into an option or shell fragment.

**Focused checks:** `cargo xtask pi probe rasputin`.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** `/dev/kvm` is unavailable without privilege escalation;
the Pi kernel cannot expose the baseline; dependency licenses are incompatible;
the dependency closure is materially larger than audited; no pinned Mac
cross-target can compile Linux/aarch64 source; or chapter 06 cannot state one
common W^X property for both hosts.

### Task V1 — introduce owned guest memory and atomic injection

**Requires:** V0; existing HVF goldens green.

**Produces:** `GuestMemory` owning allocation, checked access, mapping lifetime,
volatile scalar operations, aligned atomic words, and ownership-gated bulk
copy; host vector/status injection and guest consumption use one explicit
atomic protocol.

**Files:** `crates/wrela-vmm/src/guest_memory.rs` (new), `boot.rs`,
`exit_loop.rs`, `devices.rs`, `display.rs` or its module successor,
`stdlib/core/runtime.wr`, interrupt-cell lowering/codegen as required, chapter
06, focused unit tests, and affected golden dumps.

**Contract/dump delta:** chapter 06 states release fetch-OR/acquire
load-and-swap semantics for pending and status words. Assembly/image/report
goldens may change for that race fix; record and guest-output semantics do not.

**Work:** move raw allocation/deallocation behind RAII; centralize checked GPA
translation; preserve pool-window and display ownership checks; add aligned
atomic u32/u64 load, swap, and fetch-OR accessors with explicit ordering; make
host raises `fetch_or(Release)`; make guest checkpoint consumption acquire-load
then `swap(0, Acquire)`; apply the same no-lost-bit rule to device status;
replace the display byte loop with an ownership-gated bulk-copy boundary; and
remove duplicated raw pointer arithmetic only when the replacement is equally
strict.

**Tests:** allocation and atomic alignment; boundary reads/writes; two host
raisers setting different bits; host raise racing guest swap; device status
raise racing acknowledgement; checkpoint fast-path visibility; ownership
transfer before bulk copy; hostile concurrent-copy refusal; device-window
rejection; mapping outlives vCPUs; exact existing boot transcripts and frame
digests.

**Focused checks:** existing signed HVF smoke plus focused guest-memory tests.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** the type requires unsound `Sync`, creates a shared Rust
slice over guest-writable bytes, mixes atomic and non-atomic access to a live
pending/status word, permits a lost vector bit, weakens a device ownership
check, or changes a guest-visible result without an approved contract reason.

### Task V2 — publish normalized host types and evidence

**Requires:** V1.

**Produces:** backend-neutral capability, boot-state, exit, MMIO completion,
diagnostic, handle, and error types; the `HostBackend` trait consumed with
static dispatch and a synthetic fake backend; the exactly-once MMIO
completion token; the complete §6.1 boot/visibility state table and the
system-instruction allowlist certificate schema; stable synthetic exit
fixtures; no KVM implementation.

**Files:** `crates/wrela-vmm/src/host/mod.rs` (new), portable error definitions,
test fixtures, and this document if names differ.

**Contract/dump delta:** adds the internal normalized-exit test format; no
guest-visible dump.

**Work:** define only operations used by the current VMM; model pending MMIO
completion explicitly; model stage 1 as either the narrowly permitted
diagnostic-disabled state or the fixed compiler-owned enabled state; keep raw
backend values in diagnostics; prevent host register identifiers and handles
from entering portable state.

**Tests:** synthetic MMIO read/write completion, canceled, breakpoint, raw
timer-as-unexpected, diagnostic-disabled boot refusal outside lab mode,
enabled boot-state validation, missing completion, duplicate completion,
unexpected exit, and host-error formatting.

**Focused checks:** focused host-module unit tests.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** the API requires exposing `hv_vcpu_t`, `kvm_run`, raw
register IDs, a generic unsafe pointer, or device semantics to a backend.

### Task V3 — migrate HVF behind the host boundary

**Requires:** V2.

**Produces:** the sole HVF implementation under `host/hvf.rs`; portable engine,
scheduler, and diagnostics no longer call Hypervisor.framework directly.

**Files:** `host/hvf.rs` (new), `hv.rs` (moved/deleted as appropriate),
`boot.rs`, `engine.rs`/`scheduler.rs` (new as earned), `exit_loop.rs`, `lib.rs`,
and existing HVF tests.

**Contract/dump delta:** none; all stable outputs remain byte-identical to the
post-V1 baseline.

**Work:** move VM/vCPU/map/protect/register/run/exit calls; adapt ESR and BRK
decoding; move PC advancement into MMIO completion; replace numeric vCPU arrays
with handles; preserve watchdog and test injection behavior.

**Tests:** every existing signed HVF test; exact boot corpus; lifecycle failure
injection at VM create, map, vCPU create, register init, run, completion, and
destroy-safe unwind.

**Focused checks:** the complete signed HVF test lane.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** any golden changes without a machine reason; duplicated
portable/HVF scheduler branches; a vCPU can outlive memory; or signed coverage
falls.

### Task V4 — boot one vCPU through KVM

**Requires:** V3 and the approved V0 dependency amendment.

**Produces:** target-gated pinned KVM dependencies; a full Mac-built
Linux/aarch64 VMM artifact in the required gate; and a one-vCPU headless KVM
backend that boots an explicitly diagnostic MMU-off image through console and
guest exit. This task is a migration: it replaces the inventoried prototype,
and `boot_kvm.rs` with its manual UAPI is deleted here per the V0 inventory.

**Files:** workspace/Cargo lockfiles, cross-target configuration,
`crates/wrela-vmm/Cargo.toml`, `host/kvm.rs` (new), KVM tests, xtask
build/prepare/run support, `cargo xtask verify`, and stable conformance
fixtures.

**Contract/dump delta:** KVM appears in host identity and conformance
records; no guest-visible delta.

**Work:** select the machine IPA width at `KVM_CREATE_VM`; create the VM;
register DRAM; query `KVM_CAP_READONLY_MEM` and every used extension; create
the preferred arm64 vCPU; mask writable ID registers to the Wrela baseline in
the kernel's documented order before any other vCPU register access; validate
the host baseline without
pretending HVF can mirror KVM feature masking; set the diagnostic boot
registers; normalize MMIO; complete reads/writes; return structured errors;
cross-build and cache the binary on the Mac; transfer and run it on Rasputin;
and add the full Linux/aarch64 build to `cargo xtask verify`. If supported and
layout-safe, use a read-only text memslot as the early write-protection floor.

**Tests:** API/capability refusal; bad target; read-only-memory capability
present/absent; memory-slot rejection; entry x0, PC, PSTATE, SP, and FP/NEON
state; console/exit; 1/2/4/8-byte MMIO rejection or acceptance exactly
matching the machine protocol; unexpected KVM exits; full local target link;
and static-binary execution on Rasputin.

**Focused checks:** `cargo xtask pi run rasputin boot-basic` and the named MMIO
fault cases.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** KVM needs a GIC/firmware path; the baseline cannot be
validated before run; KVM completion cannot reproduce HVF MMIO semantics; no
pinned Mac cross-target can build the real backend; the Pi cannot execute the
cross-built artifact; or a manual Linux UAPI copy is introduced beside
rust-vmm or survives this task's close.

### Task V4a — measure and close the stage-1 machine decision

**Requires:** V4's one-vCPU KVM boot and conforming Rasputin host evidence.

**Produces:** paired MMU-off/MMU-on physical evidence; the exact common
stage-1/W^X contract in chapter 06; a stable page-table/report design; and an
explicit decision whether draft machine v1 is amended or the shared machine
revision is bumped.

**Files:** a small synthetic KVM fixture and provisional table builder, xtask
measurement support, raw result artifacts if deliberately retained, this
document, chapter 06, machine layout/report design, and focused parser tests.

**Contract/dump delta:** chapter 06 gains the exact translation granule,
reserved table range, attributes, boot register values, section-separation
rule, MMIO mappings, and permission-fault diagnostic path. Production compiler
output does not adopt them until V5.

**Work:** run fixed compute-loop and memory-copy workloads with identical
output in MMU-off and provisional MMU-on configurations; retain raw samples,
exit census, identity and profile digests, page size, kernel, temperature, and throttle
state; verify rather than tune host state; choose the common granule and table
placement; specify Normal-WB DRAM, Device MMIO, RX text, RW/NX data,
read-only table pages, and `SCTLR_EL1.WXN`; decide whether the existing
unmapped-vector diagnostic path remains normative; and approve the required
machine revision before V5 code.

**Tests:** evidence parser; mismatched-output refusal; missing capability
digest; zero samples; host-state mismatch; malformed table attributes;
executable/writable page overlap; missing MMIO mapping; and nondeterministic
table-byte rejection.

**Focused checks:** paired Rasputin runs repeated from a conforming cold host;
the result reports magnitude without promoting a predicted multiplier.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** MMU-on cannot boot through KVM; the chosen guest granule
is unsupported by either product host; MMIO cannot remain stage-2 trapped; the
original permission fault cannot be classified consistently; or no common
machine revision can express the same state on HVF and KVM.

### Task V5 — implement common stage-1 translation and W^X

**Requires:** V4a's approved chapter 06 contract and machine-revision decision.

**Produces:** compiler-emitted sealed identity tables; stable table/layout
dumps; compiler-owned stage-1 boot values; mechanical initialization in both
backends; RX text, RW/NX data, and read-only table pages; and semantic
protection-fault parity before multicore or devices proceed.

**Files:** compiler layout, image/report and dump modules, machine constants,
boot/runtime code as required, `GuestMemory`, `host/hvf.rs`, `host/kvm.rs`,
chapter 06 conformance fixtures, and affected goldens.

**Contract/dump delta:** implements the machine contract approved in V4a.
Every reserved byte, table entry, boot register, mapping attribute, section
padding decision, and revision value is stable and golden-covered before
feature work continues.

**Work:** emit deterministic page tables inside the sealed image; keep table
construction independent of host kind; validate tables and boot values before
entry; initialize the same compiler-owned EL1 registers on HVF and KVM; map
all machine MMIO as stage-1 Device while leaving the corresponding stage-2 IPA
unbacked for userspace exits; preserve HVF stage-2 protections; use KVM
read-only text memslots when capability-checked; and normalize the original
stage-1 fault class even if the host observes the later unmapped-vector abort.

**Tests:** table-byte and report goldens; table alignment and coverage; missing
or extra MMIO mapping; code/data page separation; boot on HVF and KVM;
write-to-text; execute-from-data; write-to-table; original ESR/FAR/ELR
classification; ISV=0 fail-closed behavior; and identical deterministic guest
output with stage 1 enabled.

**Focused checks:** signed HVF protection suite and Rasputin stage-1 boot plus
negative protection fixtures.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** either backend synthesizes different guest tables; a
writable page is executable; executable and writable bytes share a protection
page; MMIO becomes a guest stage-1 exception instead of a VMM exit; table bytes
are nondeterministic; or a machine change lands without chapter 06 and golden
evidence first.

### Task V6 — multicore, park, wake, and cancellation

**Requires:** V5.

**Produces:** report-declared multicore KVM execution with the shared scheduler,
deadline park, vector wake, and bounded watchdog cancellation.

**Files:** `host/kvm.rs`, portable scheduler/engine modules, KVM focused cases,
and conformance fixtures.

**Contract/dump delta:** none.

**Work:** create each vCPU on its owner thread; publish safe cancel handles;
capability-check the immediate-exit/signal path; keep atomic pending-word
injection and baton scheduling portable; handle EINTR and spurious wakes
without creating a Wrela choice.

**Tests:** N=2/N=3 release; host core refusal; cross-core admission order;
two simultaneous vector raises; raise racing acquire-swap; status raise racing
acknowledgement; park-before-wake, wake-before-park, deadline wake, watchdog,
simultaneous cancel, core fault naming, and exact core marks.

**Focused checks:** Rasputin multicore boot and park/replay corpus.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** correctness depends on an emulated irqchip; wake timing
enters record/replay; an asynchronous handle can mutate registers; or a
watchdog cannot stop every live vCPU.

### Task V7 — device and replay parity

**Requires:** V6.

**Produces:** clock, entropy, block, admission, display-headless, record, and
replay parity across HVF and KVM.

**Files:** portable engine/devices/record/display modules,
`crates/xtask/src/backend_conformance.rs` (new), and permanent fixtures.

**Contract/dump delta:** adds `wrela-backend-conformance-v1`.

**Work:** run identical artifacts on both hosts; compare semantic evidence;
record on each and replay on both; separate raw host exit census from Wrela
choices; promote every discrepancy to a focused regression.

**Tests:** full static/headless device corpus; hostile block descriptors;
entropy/clock replay; malformed record; frame digest sequence; cross-host
record/replay in both directions.

**Focused checks:** `cargo xtask pi conformance rasputin`.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** a record contains backend exits; a device branches on
backend kind; output digests differ; or a mismatch is waived as scheduling
noise without a machine-contract explanation.

### Task V8 — productionize the Rasputin runner

**Requires:** V7.

**Produces:** the productionized `wrela-lab-agent` remote helper;
content-addressed cross-built-binary deploy/run, validated result retrieval,
bounded cleanup, stable local artifact directories, and a documented
remote-build fallback that is not the ordinary path.

**Files:** `crates/xtask/src/pi.rs`, xtask dispatch/usage, parser tests, and
operator documentation.

**Contract/dump delta:** finalizes host and run metadata fields; no machine
delta.

**Work:** implement local binary caching and artifact-only deployment; unique
remote directories; digest checks; explicit timeouts;
stdout/stderr/result/record retrieval; manifest-over-stdin lab-agent
invocation with no shell-composed arguments; SFTP-only transfer; remote
deadline supervision inside the agent; safe
retain/cleanup; clear offline and stale-cache failures; and isolate the remote
source-build fallback behind an explicit option with complete toolchain
provenance.

**Tests:** fake-SSH process harness; spaces and hostile arguments; interrupted
binary transfer; wrong digest; truncated result; remote nonzero exit; timeout;
cleanup-target validation; local binary cache hit/miss and source-digest
invalidation; fallback remote-build provenance and refusal when implicit.

**Focused checks:** repeated prepare/run on Rasputin showing one local
cross-build, a binary cache hit, and image-only updates.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** remote execution requires stored credentials, `sudo`,
string-built shell commands, broad deletion, copying the whole developer
workspace on every image iteration, or ambient remote Cargo state enters
release provenance.

### Task V9 — establish Pi performance evidence

**Requires:** V8, the production stage-1 configuration from V5, and named
workloads with deterministic output.

**Produces:** `wrela-pi-benchmark-v1`, host-state refusal rules, raw samples,
and diagnostic measurements of the protected functional KVM configuration. No
product baseline or threshold is claimed before V10 closes reservation and
host isolation.

**Files:** xtask benchmark implementation, benchmark schema/parser tests,
workload declarations, and optional deliberate `bench/results/` evidence.

**Contract/dump delta:** adds the benchmark result format; does not change the
compiler cost dump or release verdict.

**Work:** delimit timed regions on the Pi; record affinity, governor,
frequency, thermal/throttle state, samples, exits, and frame digests; bind each
result to the exact canonical host identity, profile, and run-environment
records through their digests; bind it to the sealed stage-1 table digest and
named protection profile; refuse nonconforming host state; keep SSH outside
measurement; define relock discipline before adding a numeric threshold.

**Tests:** sample/median validation; throttle/governor/affinity refusal;
zero-iteration refusal; output digest mismatch; optional-counter absence;
repeat-run variance report.

**Focused checks:** `cargo xtask pi bench rasputin <declared-workload>` repeated
from a cold host state; results remain diagnostic until V10.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** network time enters a sample; the runner silently tunes
the host; output differs across samples; temperature/throttling is unrecorded;
an identity, profile, or run-environment digest is missing or inconsistent;
or a threshold is selected
from one flattering run.

### Task V10 — close product host hardening

**Requires:** V5 protection parity, V7 functional backend parity, and V9
diagnostic performance evidence.

**Produces:** committed DRAM; a minimal Linux jail and resource profile; a
signed macOS sandbox and resource profile; reproducible host identity and
profile verdicts; negative package tests; and a fresh provenance-complete benchmark of
the accepted Pi product configuration. A numeric threshold still requires
repeated measurements and a separate lock commit.

**Files:** VMM memory/backend and process-launch code, minimal Linux host
configuration, macOS signing/entitlement/sandbox configuration, packaging
manifests, chapter 06 if its host promises need clarification, and conformance
fixtures.

**Contract/dump delta:** no guest-visible delta; final host profile records
name the enforced Linux or macOS isolation profile and its provenance.

**Work:** reserve the full 512 MiB guest DRAM before entry by prefaulting and
locking the ordinary-page mapping as the single product mechanism (Rasputin's
pinned kernel has `CONFIG_HUGETLBFS` disabled); run the Linux VMM as a dedicated
user on a read-only root with mount/PID namespaces, a seccomp allowlist,
cgroup CPU/memory policy, explicit `/dev/kvm` and `/dev/dri` device access,
and no network; run the signed Mac VMM/Forge child under App Sandbox — a
separate control from the hypervisor entitlement, which is the only
entitlement currently present — with the promised filesystem, device,
network, and resource constraints;
record kernel/XNU, configuration, signing, entitlement, and package
provenance; distinguish explicitly labeled local diagnostic runs from
product-conforming ones; and fail closed when either product profile cannot be
enforced. This outlier task explicitly permits three commits — V10a the Linux
jail, V10b the macOS sandbox, V10c packaging and the provenance-complete
rerun — each passing the repository gate.

**Tests:** retained write-to-text and execute-from-data regressions; missing
memory reservation; resource-limit failure; Linux syscall/file/device escape
surface; macOS entitlement/file/network/device escape surface; unsigned or
wrongly signed child refusal; teardown after every injected failure; and
capability-profile digest mismatch.

**Focused checks:** hardened Rasputin conformance, negative package tests, and
the V9 benchmark workload rerun under the accepted Pi configuration; signed
Mac sandbox conformance runs the corresponding HVF negative suite.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** product acceptance relies on host convention rather than
enforcement; committed memory can disappear after entry; either VMM needs
persistent broad host privilege; macOS sandboxing is silently omitted while
chapter 06 promises it; or diagnostic-profile evidence is presented as
product-conforming.

### Task V11 — integrate interactive presentation

**Requires:** V7 and Pixels P8.8a–P8.8c.

**Produces:** the presenter boundary consumed by Pixels P8.8d Metal and P8.8e
DRM implementations, with headless equivalence and atomic presentation.

**Files:** `display/` modules and the files owned by the corresponding Pixels
tasks. This task is a coordination gate and is satisfied by those P8 commits;
do not create duplicate commits when both plans are active.

**Contract/dump delta:** exactly the Pixels plan's display outputs; no alternate
format.

**Work:** keep validation/digest upstream of sinks; present complete frames;
translate vsync and input through the machine devices; preserve the previous
frame on failure; keep host APIs out of portable display state.

**Tests:** headless/Metal/DRM digest identity; partial-frame refusal; presenter
failure; vsync sequence; input normalization; replay suppresses real outputs
while preserving digests.

**Focused checks:** signed Mac interactive fixture and Rasputin DRM fixture.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** a host renders or modifies a pixel; sinks consume
unvalidated guest pointers; backend kind changes a digest; or display work
forks the P8 ABI.

### Task V12 — expose the Forge-ready Mac loop

**Requires:** V3 for HVF and V11 for Metal presentation.

**Produces:** a documented local build/run/restart process contract suitable
for Forge, complete validated-frame output, normalized input, and retained
diagnostic artifacts.

**Files:** compiler/VMM orchestration code chosen by the implementation, VMM
process options or local IPC if earned, and Forge integration documentation.

**Contract/dump delta:** any local IPC gets its own versioned development-host
schema; no machine or image delta.

**Work:** build and launch from Mac source; restart on change; keep the VMM in a
separate failure domain initially; surface compiler dumps, transcripts,
records, frame digests, and host errors; do not add mutable guest state.

**Tests:** rebuild/restart; compiler failure leaves the previous run clearly
identified; VMM crash recovery; frame sequence/digest transport; input order;
stale image/report mismatch; no hot replacement.

**Focused checks:** manual Forge harness iteration plus automated process/IPC
tests.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** Forge needs a second renderer/compiler, image mutation,
host-only guest semantics, an unversioned IPC protocol, or in-process VMM
failure can corrupt editor state.

### Task V13 — seal the proxy-validation contract and calibration corpus

**Requires:** V10's accepted product host configuration and V9's benchmark
machinery.

**Produces:** the sealed §13.4 validation contract with numeric envelopes; the
calibration corpus manifests; `wrela-proxy-validation-v1` parsers and
comparison tooling; the required counter set; no proxy change and no
guest-visible delta.

**Files:** this document,
`docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md` (coordination
note), `crates/xtask/src/proxy_validation.rs` (new), corpus manifests under
`bench/`, parser tests and synthetic fixtures.

**Contract/dump delta:** adds `wrela-proxy-validation-v1`; no image, report,
machine, or record delta.

**Work:** define the predicted/measured pairing keys (proxy revision, image
digest, workload digest, host identity and profile digests); partition the
corpus into a calibration set and a frozen holdout set and seal the holdout;
seal per-class
overprediction envelopes and the measurement-noise envelope from repeated
conforming Rasputin runs; declare the kernel, frame, and sequence corpus
classes, each containing real renderer workloads rather than microkernels
alone; declare rank-fidelity candidate pairs from real compiler selection
decisions; require the named counter set for validation-class runs; and
record in the Pixels plan that P13.2 consumes this lane.

**Tests:** hostile parser cases; synthetic conservatism-violation detection;
synthetic discordant-pair computation; corpus-manifest validation; refusal of
a report missing a required counter row; envelope-provenance fields present.

**Focused checks:** dry-run comparison of a stored synthetic benchmark record
against stored proxy predictions.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** an envelope chosen without repeated-run noise evidence; a
corpus class without a real-workload member; any contract clause that lets a
measured value define a proxy transition constant.

### Task V14 — validate single-core kernel and frame evidence

**Requires:** V13.

**Produces:** the first provenance-complete `wrela-proxy-validation-v1` report
covering the kernel and single-frame classes on one vCPU under the product
protection profile; every discovered model defect closed or explicitly open.

**Files:** xtask validation runner, `bench/results/` evidence, focused
regression fixtures for every discovered model defect.

**Contract/dump delta:** none beyond V13's format.

**Work:** run the corpus single-vCPU on a conforming Rasputin host with
stage-1 enabled and the product protection profile; collect the required
counter rows host-side on the pinned vCPU thread; compare measured cycles and
counter-attributable behavior — cache refills against modeled memory-class
transitions, branch mispredicts against modeled paths — with proxy
predictions; route every conservatism violation to a published-record or
state-model fix and rerun; record the accepted report with full provenance.

**Tests:** report round-trip; nonconforming-host refusal; digest binding
between report, capabilities, image, and proxy revision; a permanent
regression fixture for each closed defect.

**Focused checks:** `cargo xtask pi validate-proxy rasputin --class kernel`
and `--class frame` (final command names may differ), repeated from a cold
conforming host.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** a conservatism violation closed by widening an envelope
in the same change; a proxy or cost-profile constant fitted from an observed
sample; a missing counter row silently tolerated; measured attribution that
contradicts the model's memory-class story left unexplained.

### Task V15 — validate the flagship topology and establish the drift lock

**Requires:** V14 and V11's DRM presentation.

**Produces:** full-frame and sequence-class validation under the sealed
flagship worker topology with active display presentation; sustained-cadence
and thermal evidence; the standing drift lock that gates every future proxy
revision.

**Files:** xtask validation runner, `bench/results/` evidence, this document,
and the Pixels plan's P13.2 consumption note if wording must be reconciled.

**Contract/dump delta:** none; the drift-lock rule is recorded normatively in
this document and consumed by Pixels P13.2.

**Work:** run acceptance-style frame and minutes-long sequence workloads under
the sealed flagship topology (the Pixels D-P8R-02 profile) with DRM
presentation active; validate the cost profile's contention and
memory-bandwidth terms against measured multi-worker behavior; record
temperature, throttle state, and sustained frame cadence; establish the drift
lock: a proxy revision may be sealed or bumped only while a current passing
report exists, a report is invalidated by any change to the referenced host
identity or profile record (kernel, EEPROM firmware, module set, hardening
profile) while run-environment records remain per-run evidence, the §13.4
offline binding check is wired into `cargo xtask verify`, and the operator
rerun policy is documented.

**Tests:** multi-worker report validation; stale-report detection when the
identity or profile digest changes; drift-lock refusal fixtures; the offline
binding check passing and failing in `cargo xtask verify`; sequence-class
thermal/throttle field presence.

**Focused checks:** `cargo xtask pi validate-proxy rasputin --class sequence`
under the flagship topology, repeated across thermal conditions.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** a contention-term conservatism violation waived as
scheduling noise; a drift-lock bypass path; a validation report older than the
identity or profile record it cites; display traffic excluded from the sequence
class to make a result pass.

## 16. Completion criteria

This plan is complete only when all of the following are true:

1. `boot.rs` and the portable engine contain no direct HVF or KVM calls.
2. Exactly one host module is selected at compile time.
3. The same image/report pair boots through HVF and KVM.
4. The compiler emits one deterministic sealed stage-1 table set and boot
   state used unchanged by both backends.
5. DRAM, MMIO, text, writable data, and page-table pages have the chapter 06
   attributes, and protection faults have one semantic classification.
6. Pending vectors and injected device status use atomic release
   fetch-OR/acquire load-and-swap without mixed non-atomic access.
7. Cross-host record/replay and device/frame digests agree.
8. Multicore park/wake/watchdog behavior has permanent KVM regressions.
9. Rasputin runs from an explicit remote command without becoming the editing
   or compiler-analysis environment.
10. The primary Pi VMM binary is cross-built on the Mac; remote building is an
    explicit provenance-complete fallback.
11. Remote results are content-addressed, provenance-complete, and parsed
   fail-closed.
12. Pi measurements exclude network time, record thermal/throttle state, and
    reference the exact accepted host identity, profile, and run-environment
    records by digest.
13. W^X and committed-memory requirements are enforceable on the product Pi.
14. Linux product execution is jailed and signed macOS product execution is
    sandboxed; diagnostic profiles are labeled nonconforming.
15. Physical baselines are rerun after the final protection and reservation
    configuration is active.
16. Headless, Metal, and DRM sinks consume the same ownership-transferred,
    validated frame.
17. Forge runs the actual guest renderer through HVF rather than a host clone.
18. `cargo xtask verify` remains local and green after every task and performs
    a full Linux/aarch64 VMM build after KVM lands.
19. Every KVM-specific discovery is promoted to permanent coverage.
20. No dependency beyond the audited, target-gated KVM allowlist was added
    without a separate approved amendment.
21. The §13.4 validation contract is sealed, and the active proxy revision
    carries a current passing `wrela-proxy-validation-v1` report with zero
    open conservatism violations across the kernel, frame, and sequence
    classes on both the calibration and frozen holdout sets.
22. Multi-worker validation under the flagship topology exercised the cost
    profile's contention terms with display presentation active, and the
    drift lock binds proxy revision, validation report, and host identity
    and profile by digest, with the offline binding check enforced by
    `cargo xtask verify`.
23. The P8-era KVM/DRM prototype's manual UAPI is deleted, and every
    surviving behavior from its V0 inventory is covered by a named fixture.

The final product remains the contract already stated in chapter 06: one
designed Wrela machine, implemented by one portable VMM with two thin host
adapters, developed where iteration is strong and measured where the appliance
actually runs.
