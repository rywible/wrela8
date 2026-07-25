# The model

## 1. Purpose

wrela builds fixed-function appliance images: sealed systems whose complete
behavior is known at build time. Its flagship product is **wrela OS** — an
appliance operating system for creatives that runs on a Raspberry Pi 5 with
1 GiB of memory, every app built in statically, nothing downloadable, with
instant boot, deterministic latency, and reproducible-to-the-pixel
execution as the claims that beat a general-purpose OS on its own hardware
class.

The stack is owned front to back: the language, the compiler and its
aarch64 backend (no LLVM, no external linker), the standard library and its
drivers, and the virtual machine the image runs on — the **wrela machine**
([06](06-machine.md)), implemented by an in-house VMM on Linux/KVM and
macOS/Hypervisor.framework. The language is designed around facts this
closed stack makes true:

- the complete code graph is known at build time;
- the actor and task set is finite and bounded;
- the machine — 3 cores, one memory map, one closed virtio device set — is
  a versioned contract, not a discovered environment;
- hardware bindings and resource budgets are build inputs; and
- no code enters the image after it is sealed.

The compilation unit is not a library or a process. It is the machine image.

## 2. The closed world

An image contains exactly the runtime code reachable from its single `@image`
entry, the wrela machine contract, and the actors, tasks, ISR bindings, and
teardown paths wired by the image. The compiler rejects a build whose
reachability is not closed. There is no dynamic loader, JIT, runtime dispatch,
`dyn` type, or unbounded task creation.

An image has one address space and the machine's three cores, each running
one cooperative event loop. Every actor lives on exactly one core, assigned
at build time; within a core nothing runs simultaneously, and across cores
the only interaction is the same typed message channels, lowered to
generated bounded rings. Placement does **not** eliminate asynchronous
interleaving, compiler reordering, DMA concurrency, or interrupt delivery;
those remain explicit parts of the model.

The result of `@image build()` is a typed graph: device bindings, actor
instances, mailboxes, pools, task slots, interrupt vectors, and the
supervision tree. Actor-to-actor edges are typed message channels, not object
references. Device edges are capabilities minted by image binding.

```text
Image
├── Target and boot contract
├── Driver actor ── owns MMIO, IRQ, DMA pools, queues
├── Storage actor ─ owns filesystem and cache
├── App actor ───── owns application state
├── Pools, mailboxes, task frames
└── Supervisor tree
```

## 3. What the language is

The user-facing language is deliberately small. A competent user holds these
concepts, and no others:

**Values.** Everything is a value, of one of two kinds: **data copies**
(like an integer — the compiler reports what that costs) and **resources
move**, every move spelled `take`. Three access modes — `read`, `mut`,
`take` — cover every use, mirrored at call sites. A resource must end
somewhere, and the compiler knows where.

**Pools.** All runtime-variable allocation comes from a bounded pool —
image-declared or scoped. `own[P] T` is a movable owned handle into pool `P`.

**Generics are structural.** A generic simply uses its parameters; every
instantiation is checked concretely, and the compiler *infers and displays*
each generic's contract. There are no interface declarations.

**Actors.** Mutable state lives in exactly one actor. Actors exchange messages
through typed async calls; one turn runs at a time. `async fn` may suspend;
plain `fn` may not. A `group` scope is the unit of deadline, cancellation,
and bounded child work. Every actor has one build-time core — inferred, or
set in the image — and nothing about the APIs changes across cores.

**Errors.** Recoverable failures are `Result` values propagated with `?`. Bugs
abandon the actor and reach its supervisor. Cross-actor calls add one composed
error type, `CallError`.

**Cleanup.** `defer` runs a registered action on every path out of a block,
in reverse order, wired into the same graph that governs cancellation.

**Drivers.** Hardware authority is an unforgeable capability held only by a
`@driver` actor. MMIO is typed, DMA ownership is tracked, and a function bound
to an interrupt vector is effect-restricted.

**Comptime.** Ordinary functions can run at build time. `comptime if`,
`comptime assert`, and the `@image` constructor specialize and wire the image.

That is the language. Chapter [02](02-language.md) specifies it, chapter
[03](03-hardware.md) covers the driver surface, and chapter
[05](05-library.md) fixes the standard-library contracts the invariants
depend on.

## 4. What the compiler is

Everything else in earlier drafts of this specification — region classes,
frame layout, mailbox capacity derivation, provenance brands, wait-for graphs,
scheduling policy, restart mechanics — is the compiler's obligation, not the
user's vocabulary. The user never names those things; the compiler proves
them, and the build report shows its work.

The compiler MUST prove, before emitting an artifact:

1. **Memory**: every allocation belongs to a bounded pool (image or scoped),
   a frame, or the image itself; total footprint has a build-time ceiling.
2. **Aliasing**: exclusive access never overlaps; no reference outlives its
   source; nothing borrowed crosses `await` or an actor boundary.
3. **Progress**: the unified wait-for graph over turns, tasks, replies,
   receipts, permits, and cleanup is acyclic; every loop is a checkpoint or
   has a proven bound; every mailbox and in-flight request count is finite.
4. **Hardware**: capabilities are never fabricated; CPU code never touches
   device-owned DMA; ISR-bound functions stay inside the ISR effect set;
   nothing device-owned is reclaimed before quiescence.
5. **Failure**: recoverable errors are values; abandonment cannot skip
   resource teardown; every restart dependency has a declared recovery source.
6. **Contracts**: the requirement set of every generic, the inferred effect
   of every private method, and every proof-conditioned form (`send`,
   `*_proven`) are computed, checked, and published — contracts are compiler
   output, not user ceremony.

Chapter [04](04-compiler.md) specifies these obligations, the required image
report, diagnostics, reproducibility, and the optimization as-if rules.

## 5. Safety claim and threat boundary

In conforming source, wrela prevents: use-after-free and double ownership;
mutable aliasing; cross-actor shared mutable state; unbounded runtime
allocation and recursion; app-level fabrication of MMIO/DMA/IRQ authority;
CPU access to device-owned DMA payloads; ISR effects outside the ISR set; and
reclaim of in-flight DMA on an unquiesced device.

This is language-level isolation in one address space, not process-style fault
containment. The trusted base is: the compiler and its generated code, the
wrela VMM and its device models, and the host kernel. The first two are
in-house and codesigned; nothing in the boot-to-pixel path is third-party
firmware, a bootloader, a GPU driver, or a vendor blob. A bug in any
trusted component can still compromise the entire image — ownership of the
stack makes the base auditable, not infallible.

The safe language has no pointer arithmetic and no `unsafe` block. Any future
FFI or unsafe facility is a separately auditable target capability outside
revision 0.1.

## 6. What "static" means

wrela promises **static bounds**, not that every value exists forever:

- image-root objects, mailboxes, task frames, pools, and baked data have
  fixed layouts;
- scoped pools hold a runtime-varying number of values up to a known cap
  and reset at deterministic points; and
- when an allocation's required lifetime forces promotion to image lifetime,
  the compiler reports the promotion and its cause.

This permits workloads like a compositor scene with a dynamic object count per
frame while retaining a build-time memory ceiling.

## 7. Revision boundary

Revision 0.1 deliberately excludes: shared-memory concurrency and
app-visible atomics (cores interact only through actor messages);
hardware-accelerated rendering (the display path is software rendering onto
the machine's framebuffer device — [06 §7](06-machine.md)); dynamic loading
and downloadable apps; garbage collection; reflection and dynamic dispatch;
nominal interface/trait declarations (generics are structural); async and
escaping closures; UEFI and any firmware boot path; non-ASCII identifiers;
and a general user-defined iteration protocol.

Each exclusion is reversible in a later revision without breaking the
model; a new device or core count is a machine revision, never an ambient
environment change.
