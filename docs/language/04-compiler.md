# The compiler contract

Everything in this chapter is invisible in source code and mandatory in a
conforming implementation. The closed world matters only if the compiler
turns it into proofs and visible bounds; none of these checks may be deferred
to "programmer discipline," and no release profile may disable them.

## 1. What a successful build proves

**Closure.** The module graph is complete; exactly one `@image` constructor
is reachable; every name and attribute resolves; every generic
instantiation satisfies its inferred structural requirement set, which is
published per generic; matches are exhaustive after specialization; every
place is definitely initialized before use; every move has one owner; every
`?` conversion resolves to the target type's `from`.

**Access.** Exclusive accesses never overlap (checked on storage paths, with
provable disjointness or rejection when indexes may collide). Closure-lent
accesses end with their call; whole-value accesses surviving `await` are
rooted at the current actor turn. Nothing lent crosses an actor boundary.

**Memory.** Every allocation has a placement (§3) with a finite capacity
contribution; task frames, stacks, pools (image and scoped), and mailboxes
have proven bounds; unbounded recursion in either the sync or async call
graph is
rejected; forced promotions to image lifetime are reported, and rejected
under `@no_promote` or an unaccounted hard `@budget`. Every data copy is
priced by the target cost model — copies above the reporting threshold (a
language-defined constant in revision 0.1) are reported, and a hard
`@budget` makes them errors; cost visibility is this
chapter's obligation, not a syntax rule. A returned literal or an `init`
body constructs in place: elision into the destination is guaranteed, never
best-effort, so no aggregate is moved by being built.

**Resources.** Every auto-reclaimable resource has one generated reclaim
destination; every protocol resource has an explicit consume/return path on
every edge; deferred actions and receipt/recovery obligations form an
acyclic cleanup dependency graph, and every `defer`'s named places are valid
at every exit it covers.

**Actors.** Every message payload is legal with a fixed layout; every
mailbox capacity is derived from the closed sender set (senders' live
task/group counts, loop multiplicities, burst bounds) or the build fails
naming the unbounded path; a statement-form `send` is accepted only where
admission is proven infallible; committed `take` arguments are irrevocable
and every return-promise is a reply type or verified `Receipt`; every
handoff-shaped driver method installs the caller endpoint atomically with
admission and every handler path performs exactly one producer transition.

**Progress.** The unified wait-for graph — actor turns, tasks, replies,
receipts and their bottom-half producers, admission slots, permits, pools,
cleanup and recovery nodes — is acyclic, except where a sealed primitive
proves an external event (timer, hardware completion) resolves a node without
acquiring anything in the cycle. Diagnostics print the full cycle. Every
async loop back edge is a checkpoint or carries a proven
`@budget(bound=...)`; every synchronous and ISR-bound loop has a proven
finite cost; a checkpoint is rejected while a non-suspend-safe access is
live. Priority and deadline inheritance are derived from the same
graph, and hard deadlines without sufficient inheritance are rejected.
Revision 0.1's synchronous discharge of the loop half is the statement
attribute `@budget(bound=N)` of [02 §8.1](02-language.md): a comptime-known
integer trip bound with a fail-closed runtime counter — not a predicted
cost model. The force-rooted runtime event-loop entries named in
[02 §8.1](02-language.md) (the per-core park/run loops that *are* this
section's cooperative scheduler) are exempt from that sync `@budget`
requirement — a trip counter is not a discharge for an intentional
unbounded park→wake loop. Async checkpoint elision under a proven budget,
ISR-bound discharge, and cycle/latency proofs remain later.

**Hardware.** Capability provenance and roles match on every hardware
operation; device and vector ownership is exclusive; MMIO partitions never
overlap; ISR-bound functions fit the ISR effect set transitively; DMA
ownership transitions are valid and no reclaim precedes quiescence; queue
capacity accounts for complete descriptor chains; every untrusted control
value used as a bound is checked.

**Specialization.** Every generic is concrete; every dispatch is resolved;
no unreachable comptime branch remains; emitted section sizes match the
report.

## 2. Scheduling semantics

The reference executor is a cooperative, priority-banded event loop (device
bottom halves; normal turns; background) — **one per core**, over the
actors placed there. There is no cross-core work stealing and no migration;
cross-core edges are the generated bounded rings of §3, and the only
scheduling nondeterminism in the whole machine is cross-core admission
order at each mailbox, which record/replay logs. These behaviors are
semantic and survive every lowering:

- admission occupies one logical mailbox slot until selection; selection is
  FIFO per mailbox by admission order;
- one external turn owns an actor until it completes, errors, or abandons;
- a ready actor's scheduling key is the priority and effective deadline of
  its FIFO head; ties break by a deterministic round-robin cursor;
- cancellation is observed at awaits and checkpoints; and
- record/replay observes the same logical admissions, turns, replies, and
  faults.

Actor addresses, slot addresses, and physical scheduler hops are not
observable. The compiler must not coalesce, drop, duplicate, or reorder
logical messages because handlers look idempotent.

Async functions lower ahead of time to state machines in statically reserved
frame slots — no boxed futures, no runtime frame allocation. `await` on a
resource awaitable consumes it. Wakes are idempotent; the runtime park
primitive has mask–arm–recheck semantics so level events cannot be lost.

Starvation across bands is not silently promised: a hard
`must_service_within` bound requires either proven arrival/work bounds or an
explicit replenishment policy, else the build is rejected or the report
declares best-effort.

## 3. Placement and promotion

Source names pools; the compiler places everything else using whole-image
escape and liveness analysis:

1. values reachable from the image graph get image lifetime;
2. locals live across `await` go to their activation's frame slot;
3. other locals live on the executor stack;
4. group-scoped values live in that group's bounded region and are
   reclaimed at its deterministic close; and
5. a value's placement cannot depend on a runtime branch — joins take the
   least common enclosing placement.

Groups and pools are one region model with two binding disciplines, and
they are deliberately not one construct. A group's region is **anonymous
and inferred**: placement fills it (rule 4 above), its close is a join —
a node in the wait-for graph, with cancellation and deadline inheritance
flowing through it — and nothing about it appears in types. A pool is
**nominal and declared**: source names it, `own[P] T` carries the
binding through signatures and across actor boundaries at compile time,
it never participates in the wait-for graph, and its reclaim obligations
are its own (a DMA pool's reclaim is gated on device quiescence, not on
scope exit; reset reuses slots without touching any concurrency
machinery). A scoped pool is a named, typed window over the same region
machinery a group gets implicitly — not a group with a budget. The two
constructs encode different promises made at different binding times;
sharing a surface would either drag pools into the progress analysis or
hide two semantics behind one keyword.

Frame layout is state-sensitive: values live in mutually exclusive
suspension states may share storage, and cheap pure values may be recomputed,
only where completion, cancellation, abandonment, and restart paths are all
proved disjoint. Every forced promotion to image lifetime is a diagnostic
with a why-chain (allocation site, escape path, footprint contribution);
silent promotion of an unbounded allocation is forbidden — if no finite bound
exists, the build fails.

Placement generalizes to the machine's three cores. Every actor gets exactly
one build-time core assignment; explicit image assignments (`core=`) are
fixed first, then the compiler places the rest deterministically from
published facts only — proved maximum uninterrupted turn work, owned image
and mailbox bytes, and reserved pool bytes — sorting by descending work,
then bytes, then canonical identity, and assigning each actor to the core
whose resulting (work, bytes) pair is lexicographically smallest. The
inputs, the inference, and the final table are published in the report and
sealed into the build identity, so placement is reproducible and load
imbalance is a build-time diagnostic, not a runtime discovery. There is no
migration and no work stealing.

Cross-core actor edges keep identical message semantics, lowered to
compiler-generated bounded SPSC rings in guest memory with sealed
publish/acquire ordering — no app-visible atomics or fences exist. A
`@driver`'s vectors, pools, permits, and recovery lanes live on its core;
there is no cross-core hardware state. Same-core edges keep every as-if
fast path of §6.

## 4. Cancellation and recovery mechanics

When a group is cancelled or expires, generated teardown, in order:
atomically closes admission (later attempts get `NotAdmitted` with payloads
returned); cancels child registrations recursively; transfers each in-flight
device receipt to a generated highest-band recovery turn on the owning
driver; quarantines affected regions and pool slots; runs every ready
cleanup node in deterministic reverse source order, leaving
dependency-blocked nodes pending; lets unrelated work run while the driver
establishes quiescence; and resolves `Cancelled` to the parent only when the
cleanup graph is empty and every child is consumed. The cancelled frame
never resumes; recovery work belongs to generated nodes, not source
destructors. Recovery turns are included in actor, budget, and wait-for
analyses.

## 5. Restart mechanics

Restart allocates nothing: frames, mailboxes, and regions are already
reserved. Generated restart stops turn selection (the bounded mailbox may
keep accepting within the proven restart window), closes the failed epoch,
runs cleanup graphs and device quiescence, returns or invalidates every pool
handle per its owner contract, resolves outstanding replies with
`PeerFailed`, clears frames only once they own nothing external, re-obtains
each resource constructor argument from its declared restart provision
(re-minted capability in a new device epoch, re-drawn pool handles, retained
immutable dependency), re-runs the actor's `init` — whose error paths are
ordinary local cleanup, nothing restart-specific — and resumes FIFO
selection. Supervision epochs, group
IDs, slot generations, and reset epochs never wrap; exhaustion escalates to
the target-fatal policy rather than reusing an identity.

## 6. Optimization: the as-if rules

The emitted image may use any representation preserving source semantics,
proofs, and observable scheduling. Performance is never a license to weaken
ownership, cancellation, DMA, ISR, fault, or capacity rules.

The compiler maintains three verified whole-image representations:
`SemanticWir` (specialized, structured operations and proofs), `FlowWir`
(typed SSA retaining ownership, capacity, wait-for, and checkpoint facts —
the only serialized IR, where ordinary optimization happens), and
`MachineWir` (AArch64 ABI, layout, and every machine-level fact). The
toolchain's **own backend** lowers MachineWir to Cortex-A76-tuned machine
code and emits the bootable image directly at the machine spec's fixed
addresses — there is no LLVM and no external linker anywhere in the output
path (rustc's own build of the toolchain is the only place LLVM exists,
outside every artifact this contract covers). One CPU means the cost model
that discharges `@budget`, checkpoint, and frame proofs is the real
microarchitecture, not an abstraction. NEON code generation for the stdlib
vector types ([05 §8.1](05-library.md)) is a first-class backend
obligation — the flagship's compositor is its hottest loop. Safety
validation runs on SemanticWir and FlowWir; every backend fact (aliasing,
ranges, alignment) must trace to a semantic proof — never be invented from
naming or optimism. That proof-tracing discipline is what substitutes for
a third-party backend's soak time.

**Actor as-if.** A call through an actor handle is always a logical admission
under capacity and FIFO rules. Subject to that, the compiler may use direct
placement, specialized dispatch tables, direct reply writes, tail-continuation
forwarding, and handler fusion — provided admission order, non-reentrancy,
priority/deadline choice, checkpoints, cancellation, abandonment
attribution, and record/replay events are all preserved. Anything unproved
falls back to the ordinary queued path; source correctness never depends on
fusion.

Whole-image specialization (monomorphization, constant propagation from
image/target config, dead code and data elimination, bounds-check discharge
through FlowWir proofs, scalar replacement, frame-storage coloring, mailbox
representation by per-method banks instead of padded rings) is expected.
Checked arithmetic keeps its failure contract through the backend. Typed
MMIO, `DmaShared`, `InterruptCell`, and DMA transitions constrain motion; an
alias proof never erases a hardware observation or barrier. An optimization
used to satisfy a hard layout or timing assertion becomes a required,
verified part of that build.

## 7. The image report

Every successful build emits a machine-readable report and a summary. At
minimum: build identity (compiler, revision, target, the build-affecting
constants — quotas, thresholds — and digests of every input); memory by owner and site with peak ceiling; every promotion
with its why-chain; per-actor mailbox logical capacity and physical bytes;
frame slot counts, sizes, and overlays; stack bounds (executor, ISR, fault);
pool capacities (image and scoped) with reclaim destinations and restart
provisions;
queue shapes and maximum in-flight operations; every logical actor edge and
its physical lowering (queued / direct / forwarded / fused); checkpoint
sites and proven elisions; maximum interrupt-masked interval; receipt
handoff edges with their recovery nodes; every data copy above the
reporting threshold, with site and size; baked artifact hashes;
and code and data size by owner.

Tooling (hover, expanded views) must display inferred facts wherever source
omits them: receiver effects, pool names, reclaim classification, copy vs
move at each binding, actor edge vs internal call, suspension points and
frame fields, the ambient group lineage of every `async fn`, the inferred
requirement set of every generic, which driver methods carry the handoff
convention, and DMA state around protocol calls. Inference reduces
annotations; it must not hide causality.

Diagnostics carry a stable category, a primary span, the inferred fact that
caused rejection, a why-chain for whole-image analyses, and a source-level
repair when one exists — for example:

```text
error[wait-cycle]: blocking resource cycle
  Storage.turn -> child[0] -> Logger.turn -> Storage.turn
  note: Storage retains its non-reentrant turn while joining child[0]
  help: make the notification one-way, return a receipt, or merge the state

warning[performance]: loop may perform 4096 cross-actor turns per request
  call edge: Router.route -> Firewall.classify
  help: batch, compose into one actor, or move a bounded group through a pool handle
```

## 8. Reproducibility and phases

Identical declared inputs, compiler revision, machine revision, and
quotas produce a byte-for-byte identical unsigned image and report. No
timestamps, host paths, or undeclared environment data enter the artifact.
Pure comptime results may be cached content-addressed; a hit must be
observationally identical to evaluation. The same discipline extends to the
shipped appliance triple — host image, VMM, wrela image — so
reproducibility covers everything a device runs ([06 §9](06-machine.md)).

The logical build phases (implementations may fuse them when results and
diagnostics are equivalent): parse and collect; resolve signatures and
comptime code; evaluate the image constructor under quota; instantiate and
check specialized bodies to a fixpoint; close semantic graphs and validate
every §1 invariant; infer placement and resource bounds; lower through
FlowWir/MachineWir, lay out the image, and produce the report; run
`@layout_assert` (read-only — failure is a build error, never a second
layout pass); emit and link, verifying section sizes against the report.

## 9. Boot

The single target is the wrela machine ([06 §3](06-machine.md)): the VMM
consumes the image report, preconfigures every declared device and
shared-memory window, loads the image at the fixed base, and starts vCPU 0
at the entry — no firmware, no discovery, no UEFI. The generated entry
validates the machine revision and info page, installs per-core state,
releases the remaining vCPUs, mints capabilities, initializes drivers and
actors in image dependency order, opens mailboxes atomically, and enters
the per-core event loops. Comptime moves deterministic construction out of
boot; device reset, feature verification, and secret provisioning remain
runtime boot work.

## 10. Record/replay

Determinism is a machine property, not an optional profile: the VMM injects
vectors only at compiler-emitted checkpoints and records at the virtio
boundary ([06 §8](06-machine.md)). Record mode captures every
nondeterministic input: device completions and DMA-written bytes, each
vector raise with its consuming checkpoint, clock observations, entropy,
cross-core admission order per mailbox, and digests of every externally
visible output — including display frames, so replay reproduces what the
user saw pixel-exactly. Ordinary code cannot read raw cycle counters or
entropy — only recording capabilities. Replay feeds recorded inputs from
virtual device models, suppresses real outputs, and diagnoses any
divergence in inputs, outputs, or checkpoint order. Log buffers are bounded
pools with a declared full-buffer policy (stop with marker, stream, or
abandon) — never silent drops. The
profile must also expose cleanup-graph states (quarantine, pending nodes,
receipt transfers, mailbox reopening) as first-class replay events, so a
replay viewer can answer "why is my request not resolving."

## 11. Conformance

A toolchain conforms only if it implements every normative rule in chapters
01–06 for the machine revision it advertises. The digest-pinned test runner
is the wrela VMM itself (QEMU stands in only during bootstrap and is then
retired); machine implementations additionally pass the machine conformance
suite of [06 §10](06-machine.md). The archived draft's enumerated test
catalogue ([archive](../archive/v0.1-draft/08-build-contract.md)) remains
the working checklist; the worked example
([virtio_storage.wr](examples/virtio_storage.wr)) is a required
integration-shape test once the corresponding library APIs exist.
Structural properties (direct calls, static frames, scoped-pool reset,
exitless I/O) are not benchmark claims. The flagship's claims are named
measurements against a general-purpose OS on comparable hardware: cold
boot time, input-to-photon latency, sustained frame time under load,
zero memory-pressure terminations, and instant resume — each measured on a
named workload before it is advertised.
