# wrela language documentation

wrela is a language for sealed, fixed-function appliance images: the
compilation unit is the bootable machine image, the whole code graph is
closed at build time, and everything — memory, tasks, mailboxes, DMA — has a
proven build-time bound. Its flagship product is **wrela OS**, an appliance
operating system for creatives on a Raspberry Pi 5 with 1 GiB of memory —
all apps built in statically, software-rendered, instant-on, and
reproducible to the pixel.

## Reading order

| Chapter | Audience | Contents |
|---|---|---|
| [01 — The model](01-model.md) | everyone | Purpose, closed world, guarantees, and the language/compiler split. |
| [02 — The language](02-language.md) | users | The complete user-facing language. |
| [03 — Hardware](03-hardware.md) | driver authors | Capabilities, MMIO, DMA, ISRs, receipts, reset. |
| [04 — The compiler contract](04-compiler.md) | implementers | Every proof, inference, report, and as-if rule. |
| [05 — Library contracts](05-library.md) | both | Standard types the invariants depend on. |
| [06 — The wrela machine](06-machine.md) | implementers | The versioned virtual platform: CPU, boot, interrupts, devices, hosts. |
| [examples/virtio_storage.wr](examples/virtio_storage.wr) | both | Worked end-to-end appliance (aspirational). |

The organizing rule: **chapter 02 is everything a user must hold in their
head; chapter 04 is everything the compiler must prove so that chapter 02
stays small.** If a concept appears in 04, source code never spells it.

## Platform decisions (2026-07)

The project targets one designed machine, front to back:

- **One machine** ([06](06-machine.md)): 3 vCPUs at an ARMv8.2-A/NEON
  baseline (Cortex-A76 cost model), one memory map, a closed virtio device
  set whose drivers ship with the stdlib. New hardware is a machine
  revision, never a discovered environment.
- **Hosted, not bare metal**: a Firecracker-class Rust VMM on Linux/KVM
  (the Pi 5 flagship host) and macOS/Hypervisor.framework (development and
  Mac hosts). QEMU is bootstrap-only, then retired.
- **Direct boot, no UEFI**; paravirtual interrupts injected only at
  compiler checkpoints (no emulated GIC); shared-memory doorbells — exits
  and power are codesign outputs, and determinism is a machine property.
- **Software rendering**: CPU/NEON compositing in the guest; the display
  device is a zero-copy, tile-scatter-gather framebuffer. Every frame is
  replayable.
- **Multicore is normative**: every actor has a build-time core; APIs are
  identical across cores; cross-core parallelism is ownership transfer.
- **Own toolchain**: no LLVM, no external linker — an in-house
  MachineWir→A76 backend emits the bootable image directly, under the
  proof-tracing discipline of [04 §5](04-compiler.md).

## Relation to the earlier draft

This documentation replaces a ten-chapter draft (≈6,300 lines) preserved
verbatim in [`docs/archive/v0.1-draft/`](../archive/v0.1-draft/). The
guarantees are unchanged; the user-facing concept count is roughly a third of
the draft's. The deliberate language changes:

| Draft concept | Replaced by | Now in |
|---|---|---|
| `view` / `mut view`, `projection` + `yield`, projection carriers, ephemeral-type taxonomy, provenance rules | Non-escaping closures as the scoped-access mechanism (`fn entry[R](mut self, key, body: fn(mut Item) -> R)`) — no reference types, no lifetime rules | 02 §8.3 |
| `iso[P] T`, `brand` declarations, generative brand theory, five named region classes | Pools + `own[P] T` handles; `pool Name` bound once by the image; all other placement is compiler inference | 02 §4, 04 §3 |
| `AsyncExit[E]`, `ActorCallError[E]`, `AdmissionResult`, ownership-conditioned second-class carriers, lazy `try send` argument evaluation | One composed `CallError[E]`; admission failure hands moved payloads back inside an ordinary error value | 02 §9.4, 05 §2 |
| Four value classes (implicit-copy / explicit-copy / reclaimable-linear / strict-linear) | Data vs resources; how a resource may end is derived from its type, not declared | 02 §3.1 |
| `scope` with `abort` clause and pre-enter proof obligations | Pre-`enter` code is read-only; staged acquisition is nested scopes; `abort` is gone | 02 §10 |
| `isr fn` third function color, `@isr_safe` | A plain `fn` bound to a vector; the binding triggers transitive ISR effect checking | 03 §6 |
| `@app` / `@service` roles | One `@actor` role (plus `@driver`); app/service wiring policy dropped | 02 §9.1 |
| Match/if tail expressions, inline `if` expression, `pass`-poisoning rules | Statements only; conditional values assign per arm under definite-initialization | 02 §8.1 |
| Label-required parameters with unary exception and `_` opt-out | Labels always allowed, never required; effect mirroring unchanged | 02 §5.1 |
| Unicode identifier apparatus (NFC, confusables, bidi controls) | ASCII identifiers in 0.1 | 02 §1 |
| `Bytes`/`Str` shape-type theory | Bounded types; a parameter may omit its bound | 02 §6.2 |
| Cross-package `@comptime` contracts | Deleted — revision 0.1 has exactly one acquirable package | 02 §12 |
| Normative multicore placement semantics | Out of the normative spec until a multicore profile is advertised | 01 §7 |
| View-based container iteration (`items()`, `values_mut()`) | Owned iteration with `for` (keys, consuming arrays); lent iteration with closures (`each`, `get_mut`) | 05 §7 |
| Nominal interfaces (`interface`, `impl`, `implements`, orphan rules, coherence checking) | **Structural generics**: every instantiation is checked concretely and the compiler infers and *displays* each generic's contract — contracts are compiler output; operators and `?` are method conventions | 02 §7.3–7.4 |
| `scope` declarations (`enter`/`abort`/`exit`) | `defer`, wired into the existing cleanup graph; `with` keeps only the two intrinsics | 02 §10 |
| `request` scopes and `nursery` scopes | One `group` scope: optional deadline, optional child capacity, one cancellation domain | 02 §9.5 |
| `send` / `try send` pair with lazy-argument rules | One `send`: a statement exactly where admission is proven, otherwise a `Result` with payloads returned in the error | 02 §9.4 |
| Three closure flavors (sync, `async \|`, escaping `take \|`) | One: synchronous and non-escaping; work that outlives a call is a named function | 02 §8.3 |
| `@receipt_handoff(input=...)` annotation | Inferred from the signature shape (one `take p: P`, result `Receipt[P]`) | 03 §5 |
| Protocol bring-up `with` scopes | Fallible transitions consume their state; generated cleanup reclaims the capability on failure | 03 §9 |
| Arenas as a separate construct | The scoped lifetime of the one pool concept (`with pool(...)`) | 02 §4 |
| `Untrusted` / `Validated` / `Secret` as three features | Three instances of one marked-value mechanism | 03 §8 |
| `shadow`, `loop`, membership `in`/`not in` | Deleted (rename, `while true`, `contains`) | 02 §8 |
| Three copy classes, `copy` expression, `copy struct`, move-by-default assignment | **Data copies; resources move** — every resource move spelled `take`, everywhere; copy costs tracked, reported, and budgeted by the compiler | 02 §3, 04 §1 |
| Bespoke `init` rules (partial-`self` tracking, special `Err` rollback) | `init` kept, but its fields check exactly like uninitialized locals and `Err` rollback *is* the local-cleanup rule — colocation without a second analysis | 02 §7.1 |
| `as` cast operator (plus `checked_as`/`truncate_as`) | Conversions are the one method shape: `to[T]()`, `checked_to[T]()`, `truncate_to[T]()` | 02 §6.1 |
| Prefix `comptime` expression | `const` initializers and existing comptime contexts already force build-time evaluation | 02 §12 |
| Two import forms (`from`/`import as`) | One: `from path import name`, where a name may be a submodule | 02 §2 |
| `@dma` / `@mmio` / `@wire` | One `@layout(kind, ...)` with kinds `dma`, `mmio`, `wire`, `runtime` | 03 §3 |
| `deriving(Eq)` | Structural `==`/`!=` is automatic for every data type | 02 §7.5 |

Two candidates were cut and deliberately **restored** after review: `init`
(colocating construction with its type earns its keyword — and it no longer
carries special-case analysis) and the wrapping operators `+% -% *%` (denser
and faster to grok than method names in exactly the ring/counter/hash code
that wants them). Elegance is not minimality for its own sake.

Unchanged and load-bearing: `read`/`mut`/`take` with call-site mirroring,
checked arithmetic, actors and non-reentrant turns, bounded mailboxes,
`Receipt` and the handoff convention, ambient group lineage and structured
cancellation, typed MMIO / DMA ownership / marked values, comptime and the
`@image` constructor, image failure policy (`img.on_failure`), the image
report, and deterministic record/replay.

The draft's normative EBNF, conformance-test catalogue, build-phase detail,
and scenario schemas remain available in the archive; they are implementer
material and will be re-issued against this surface as the reference
implementation catches up.

## Status

Design documentation. The reference toolchain implements a small vertical
slice; treat every chapter as normative intent, not a boot claim.
