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
graph is rejected; forced promotions to image lifetime are reported (a hard
refusal of promotion is `@layout_assert(region_promotions == 0)`). Every
data copy is priced for the report by the language's copy-pricing rules —
copies above the reporting threshold (a language-defined constant in
revision 0.1) are reported; a hard budget on copies is a future cost-proof
intention. Separately, emitted instruction streams are ranked under a
versioned ISA proxy-cycle model ([§5](#5)); that ranking is not copy
pricing and does not discharge `@budget`. Cost visibility is this
chapter's obligation, not a syntax rule.
A returned literal or an `init` body constructs in place: elision into the
destination is guaranteed, never best-effort, so no aggregate is moved by
being built.

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

**Progress.** Progress is proved constructively, per wait-edge kind — there
is no unified wait-for-graph analyzer. Downward edges are handles; **upward
edges are resolutions or vectors**: a client reaches a driver by handle
(request); the driver answers by resolving the client's parked call or
receipt (reply); the device reaches the driver by vector
([06 §7](06-machine.md)). The ISR gate may respell the device→driver edge; it
must preserve or replace this upward-edge rule.

Disposition of every FlowWir suspension kind (`AwaitKind`):

- **Actor-call await.** Cycles only if handles cycle. Handles are a DAG by
  [02 §12.1](02-language.md) (construction and handle edges alike) plus the
  `Actor[T]` mobility class (`!crosses_actor`, never rebound —
  [05 §2](05-library.md)).
- **Group join.** Reduces to the same handle-DAG case: a join waits on
  children whose outbound edges are themselves actor-call or receipt awaits.
- **Receipt await.** Rides device progress under the sealed external-event
  carve-out (a timer or hardware completion resolves the node without
  acquiring anything that could close a wait cycle) and the
  deadline/quarantine backstop ([03 §9](03-hardware.md)).

Non-suspension edges earlier drafts put on a wait-for graph:

- **Admission** is fail-fast `NotAdmitted`, never a wait
  ([02 §9.4](02-language.md)).
- **Permits and pools.** No blocking acquire exists: `reserve` is
  synchronous fail-fast-or-proven ([03 §4](03-hardware.md)), and a driver
  never awaits a permit its own bottom half produces. A hand-spelled permit
  retry loop dies by the loop-discharge theorem below plus turn
  non-reentrancy ([02 §9.2](02-language.md)) as a budget-trip abandonment,
  not a deadlock.
- **Cleanup and recovery** run generated teardown on the owning driver's
  existing bottom-half `@task` ([§4](#4-cancellation-and-recovery-mechanics));
  they do not introduce a new wait edge among actors.
- **Starvation** is discharged by FIFO-per-mailbox + round-robin
  ([§2](#2-scheduling-semantics)) plus the same loop theorem.

Every async loop back edge is a checkpoint or carries a proven
`@budget(bound=...)`; every synchronous and ISR-bound loop has a proven
finite cost; a checkpoint is rejected while a non-suspend-safe access is
live. Revision 0.1's synchronous discharge of the loop half is
[02 §8.1](02-language.md)'s loop-discharge theorem: `@budget(bound=N)` (a
comptime-known integer trip bound with a fail-closed runtime counter — not
a predicted cost model) **or** observation on every path from the loop head
to its back edge (pending-vector read, park-page write, or a call whose
inferred **observes** bit is set). The cooperative per-core park/run loops
discharge by observation, not by name. When a proven `@budget` makes an
async loop's checkpoints elidable, the report discloses that optimization;
it is not a semantic — the legality rule (budget or suspension) is
unchanged. ISR-bound discharge and cycle/latency proofs remain later.

A future construct that would rebind a handle or introduce a blocking
acquire is refused fail-closed, naming this progress rule.

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

The reference executor is a cooperative event loop — **one per core**, over
the actors placed there. Scheduling is FIFO per mailbox and round-robin
across ready actors: there are no priority bands. There is no cross-core
work stealing and no migration; cross-core edges are the generated bounded
rings of §3, and the only scheduling nondeterminism in the whole machine is
cross-core admission order at each mailbox, which record/replay logs.
Admission order is total per core. These behaviors are semantic and survive
every lowering:

- admission occupies one logical mailbox slot until selection; selection is
  FIFO per mailbox by admission order;
- one external turn owns an actor until it completes, errors, or abandons;
- ready actors are selected by a deterministic round-robin cursor across
  actors with a non-empty mailbox;
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
and inferred**: placement fills it (rule 4 above), its close is a join
whose wait disposition is the group-join case of §1's progress theorem,
with cancellation flowing through it — and nothing about it appears in
types. A pool is
**nominal and declared**: source names it, `own[P] T` carries the
binding through signatures and across actor boundaries at compile time,
it never participates in a wait edge, and its reclaim obligations
are its own (a DMA pool's reclaim is gated on device quiescence, not on
scope exit; reset reuses slots without touching any concurrency
machinery). A scoped pool is a named, typed window over the same region
machinery a group gets implicitly — not a group with a budget. The two
constructs encode different promises made at different binding times;
sharing a surface would either drag pools into the progress theorem or
hide two semantics behind one keyword.

Frame layout is state-sensitive: values live in mutually exclusive
suspension states may share storage, and cheap pure values may be recomputed,
only where completion, cancellation, and abandonment paths are all proved
disjoint. Every forced promotion to image lifetime is a diagnostic
with a why-chain (allocation site, escape path, footprint contribution);
silent promotion of an unbounded allocation is forbidden — if no finite bound
exists, the build fails.

Placement generalizes to the image's N cores (`0 .. N-1`, sealed by
`Image(..., cores=N?)`; default N = 1). Every actor gets exactly one
build-time core assignment; explicit image assignments (`core=`) are fixed
first (`core ≥ N` is a build error), then the compiler places the rest
deterministically from published facts only — proved maximum uninterrupted
turn work, owned image and mailbox bytes, and reserved pool bytes —
sorting by descending work, then bytes, then canonical identity, and
assigning each actor to the core whose resulting (work, bytes) pair is
lexicographically smallest. The inputs, the inference, and the final table
are published in the report and sealed into the build identity, so
placement is reproducible and load imbalance is a build-time diagnostic,
not a runtime discovery. There is no migration and no work stealing.

Cross-core actor edges keep identical message semantics, lowered to
compiler-generated bounded SPSC rings in guest memory with sealed
publish/acquire ordering — no app-visible atomics or fences exist. A
`@driver`'s vectors, pools, permits, and recovery lanes live on its core;
there is no cross-core hardware state. Same-core edges keep every as-if
fast path of §5.

## 4. Cancellation and recovery mechanics

When a group is cancelled or expires, generated teardown, in order:
atomically closes admission (later attempts get `NotAdmitted` with payloads
returned); cancels child registrations recursively; delivers each in-flight
device receipt to the owning driver's existing bottom-half `@task`;
quarantines affected regions and pool slots; runs every ready cleanup node
in deterministic reverse source order, leaving dependency-blocked nodes
pending; lets unrelated work run while the driver establishes quiescence;
and resolves `Cancelled` to the parent only when the cleanup graph is empty
and every child is consumed. The cancelled frame never resumes; recovery
work runs on that `@task`, not in source destructors, and is included in
actor and budget analyses and §1's progress theorem.

## 5. Optimization: the as-if rules

The emitted image may use any representation preserving source semantics,
proofs, and observable scheduling. Performance is never a license to weaken
ownership, cancellation, DMA, ISR, fault, or capacity rules.

The compiler maintains three verified whole-image representations:
`SemanticWir` (specialized, structured operations and proofs), `FlowWir`
(typed SSA retaining ownership, capacity, progress, and checkpoint facts —
the only serialized IR, where ordinary optimization happens), and
`MachineWir` (AArch64 ABI, layout, and every machine-level fact). The
toolchain's **own backend** lowers MachineWir to AArch64 machine code
(tuned for the flagship Cortex-A76 schedule story) and emits the bootable
image directly at the machine spec's fixed addresses — there is no LLVM
and no external linker anywhere in the output path (rustc's own build of
the toolchain is the only place LLVM exists, outside every artifact this
contract covers). One sealed ISA baseline means every conforming host runs
the same words; the flagship A76 is the microarchitecture story for proofs
that need one (`@budget`, checkpoint, frame) — not an abstraction and not
the ranking ruler below. NEON code generation for the stdlib vector types
([05 §8.1](05-library.md)) is a first-class backend obligation — the
flagship's compositor is its hottest loop. Safety validation runs on
SemanticWir and FlowWir; every backend fact (aliasing, ranges, alignment)
must trace to a semantic proof — never be invented from naming or optimism.
That proof-tracing discipline is what substitutes for a third-party
backend's soak time.

**Proxy-cycle ranking.** After codegen (runtime included), every emitted
word carries an emit-time op-class tag (`CostRule`) and its dest/src
registers (loads/stores may also carry a `MemRef`: Stack vs Cold). A
versioned parameter file (`a76-pi5`, schema `version=3`) gives, per
**instruction group** of the published A76 tables, a latency, a
throughput, and the execution ports the group issues to, together with
the pipeline set and dispatch constraints, real cache and TLB geometry
with associativity and its pinned leaf latencies, the branch and
alignment terms, the declared cross-core terms, and the residual sweep
box — every row carrying its provenance tier and its source. The load
latency it pins is the **first-level hit** path only; the miss path is the
memory model's. The store latency it pins is to the **store buffer**, and
a store splits into an address operation and a data operation that
occupies a vector pipe, so store-heavy streams contend with FP/ASIMD.
Groups the emitted stream does not contain are **absent**, not
speculatively priced. A register scoreboard over the final stream yields
a **schedule length** per function — the proxy total (`call` clears the mem window). Totals are
path-insensitive Σ per function. Each function also carries its emitted
**word count**, the static footprint proxy. The value is
**differential**: given two
semantically equivalent emissions, which ranks lower? Absolute cycles on
Apple vs Pi (or any host) may differ with cache and µarch; rank for
fewer/cheaper ops and shorter true data deps must not require those.
**Proxy soundness (normative):** a proxy win must never imply a
real-machine loss (same or better only); when unsure, prefer under-credit
/ over-cost. Three consequences are normative rather than advisory,
because each names a way the cycle number alone can improve while the
machine does not. (i) The scoreboard reorders only within a **bounded
window** over a core that reorders further, so a candidate may not buy
modelled schedule with real instructions the score does not account for.
The I-side term is that account: hot-text footprint and instruction-side
translation are priced, so growth is **priced, not refused**. Emitted
word count is therefore a **reported column**, not a veto, and the hard
constraint standing in its place is the **per-core hot-text budget** —
a core's hot text is charged against its 64 KiB 4-way L1I, its 48-entry
L1 I-TLB, and the 1280-entry L2 TLB, each computed per core from sealed
placement ([§3](#3)), because that is the denominator the machine
actually has. Word growth inside those budgets is an ordinary priced
trade; growth that breaks one is refused as before. The word veto
retires only together with that budget term, never ahead of it.
(ii) A measured hit whose method has no scored function
is charged at the **maximum function schedule in the program**, never
zero: dropping a key (rename, outline, fusion) must cost, or the ruler
rewards measuring less. (iii) Measured **coverage may not fall** between
baseline and candidate. The unified cost is
`cost(P, W) = Σ_b f_W(b) × s(b)` — schedule length `s(b)` times a
workload frequency `f_W(b)`. The **flat** workload `W_flat` is policy
`f≡1` (every static word / block once); dump Assumptions still print
`valid_for=static_shape_opts` and `workload=flat` for that row. Named
workloads are pinned in `bench/workloads.toml` (required `[flat]` weight
plus measured names such as `[boot-actors]`); the dump header carries
that file's digest. Measured rows compose `Σ_b f(b)×s(b)` at **block**
grain — the requirement, not an eventual refinement: a method-grain sum
prices a hot loop body the same as the function that contains it. The
bridge from a measured block id to the emitted-word range it scores is
**proved, not assumed** — the two partitions must agree, and a mismatch
is an error, never an attribution to the nearest offset. Measured rows
print a nested
`coverage=matched/total` — **coverage honesty:** uncovered hits shrink
the matched side, must not be silently dropped, and are charged at the
program's maximum function schedule so that losing coverage can never
read as cheaper; `W_flat` remains the mass/coverage backstop. Comparing
two sides whose coverage denominators differ is an error, not a rank:
they were measured against different frequency vectors.
Frequency measurement has three lanes: **Lane 1**
scheduler method/turn counters in the guest runtime transcript; **Lane 2**
test-only in-guest basic-block hit counters (`--block-count`),
instrumenting `app`, `runtime`, and `driver` code alike, since the
generated runtime is the largest scored owner and an app-only `f` would
explain almost none of it; **Lane 3**
host/VMM agreement that Lane-2 vectors match on a named control case.
Lane 2's normative sink is the **host memory snapshot** taken after halt,
not the guest transcript: the machine's console is a fixed, statically
bounded surface ([02](02-machine.md) §12.2) and the number of hit blocks
has no static bound, so the transcript's hit line is a **bounded
diagnostic** — it carries at most a fixed number of pairs and must then
report the count it dropped, so a dropped pair is loud rather than
silent. A frequency vector is read from the snapshot. Lane 3 agreement is
therefore over **the pairs the transcript actually carries**, and the
dropped count must be independently accounted for by the snapshot, so a
truncated line can never agree by being compared with itself.
A measured block is identified by function key and block index within
that function, never by a whole-program counter: the instrumented image
and the scored program are different closures with disjoint counter
spaces, so a raw index resolves against the wrong program. That identity
is resolved to a word range offline, and the resolution is the bridge
that must be proved rather than assumed.
Widening what is instrumented never narrows what is scored: the coverage
denominator is the **whole scored set**, never the instrumented subset —
redefining it to the measured subset is the same "reward measuring less"
failure as dropping an uncovered hit. A measured block whose function is
absent from the scored program is an **uncovered** hit, charged at the
program maximum like any other; when the measured image is much larger
than the scored closure, that term may dominate the row, and the honest
response is to report the coverage fraction, never to narrow what was
measured.
Static-shape opts (delete or shorten the stream without changing dynamic
shape) may land on the flat land-gate alone. Frequency-dependent opts
(guard, outline, specialize, unroll-as-dynamic-win) land only under
**veto-then-rank overall** across the pinned set: veto if any non-flat
measured W rises (ε=0) at any point of the residual box below, if any
measured coverage falls, or if any core's hot-text / I-TLB / L2-TLB
budget is exceeded; else rank by the weight-mean of relative deltas
`(cand−base)/base` — never by device-wait wall timing. Every veto reason
that fires is reported, not just the first, and a reason that fires at
one point of the residual box names that point. **The A76 port map is the
model.** The documented Cortex-A76 execution pipelines, its dispatch
constraints, its per-group latency and throughput, and real L1/L2/L3 and
TLB geometry with associativity are all modelled, for exactly one
profile — `a76-pi5` (Cortex-A76 / BCM2712 / Raspberry Pi 5). There is no
second profile and no host profile; specificity to the flagship is the
point rather than a limitation. Fidelity is to the **published record**,
never to silicon this project measured: every pinned row carries a
provenance tier (vendor-normative for this core, vendor-descriptive,
third-party measured on this board, third-party measured on generic
silicon, unresolved), an **unresolved row may not be pinned at all**, and
the dump prints a provenance digest over the tier mix so the model can
never quietly rest on a guess. **No hardware measurement is ever built
for cost purposes** — no counter reads, no address attribution under
replay, no predicted-versus-physical agreement report — because the
published record already carries the measurement and reproducing it is
not this project's work. The model is still **not calibrated to host wall
clocks**, takes **no host PGO as a gate input**, and does not discharge
`@budget` or cost proofs. Flat `issue_width`-only scoring is not the live
model. **Residual uncertainty is swept, not chosen.** Where the record
gives a bracket rather than a number, or gives nothing at all, the pinned
value is the bracket's **pessimistic end** and the bracket itself becomes
a sweep dimension: the model **rounds toward over-costing at every
residual uncertainty**, and an unmodelled mechanism never becomes a
discount. A candidate must win at **every** point of the residual box
(∀); no `∃`-form win predicate exists, because that form is a search for
a flattering assumption rather than a gate. That rule has a second
clause, since a larger charge is not automatically the safe end for a
term whose *removal* is the win: a barrier charge must never make barrier
**removal** profitable. Pinned dumps stay single-valued at `a76-pi5`; the
sweep belongs to the land gate and never enters a pinned dump. The model
is a **scoreboard with a real port map, not a cycle simulator** — no
predictor state machine, no cache-line or coherency protocol simulation,
no prefetcher model, no simulated reorder buffer. Reuse distance plus
associativity yields a hit/miss verdict, measured branch bias yields a
mispredict estimate, sealed placement yields a local/remote verdict, and
the reorder window is a bounded depth. The hardware prefetcher is
deliberately **unmodelled**, which charges strided loads full miss cost
and therefore **under-credits** any transformation that improves stride
regularity: a stated bias in the conservative direction, never a
discount, and named here so a stride opt that scores poorly is read
against this sentence rather than believed. Every cost the emitted stream
can incur is **modelled, swept, or omitted with a written reason** —
omission by oversight is a defect, and adding a dimension means adding
its row. Stable dump: `wrela dump --stage=cost` (Terms = rule counts, plus
schedule totals, owners, and `Workload` rows). The image report carries
only a short summary ([§6](#6)). **Ruler oracles (normative):** the
ranking is itself under test, not only the code it ranks. A semantically
neutral change must never rank as a win (**null-opt** — renaming every
scored function key is the canonical case, since it is exactly the shape
fusion and outlining take), and adding a dead instruction must never
lower a schedule (**monotonicity**). A ruler that fails either is wrong
whatever it says about the emissions under it. This ranking is the
optimization ruler (M18); modes below consume it and do not replace it.

**Compile modes.** The compiler has exactly two product modes: `dev`
(every named optimization off) and `release` (every named optimization
on). The default product path is `release`. Each optimization is an
ordinary named function in a fixed in-code call order — skippable by
mode, never a recipe file, evidence table, or plugin. Profitability is
scored only under the proxy-cycle ranking above, always **in context** of
the full pipeline. For the fixed cost-* corpus under `W_flat`, a
candidate pipeline must not raise any case's flat proxy total, must not
break any core's hot-text / I-TLB / L2-TLB budget, and must strictly
lower at least one proxy total (static-shape land-gate). Emitted word
count is reported per case and no longer a condition of its own: with the
I-side footprint priced, a static-shape opt that grows the stream is
argued on the budget rather than refused for the growth alone. When
measured
workloads are in scope, the overall gate is veto-then-rank across the
pinned `workloads.toml` set as above. Losers are deleted or reworked,
not kept disabled. Host wall-time, flame graphs, `profile`, and
`bench guest` A/B are out of this process — optional offline research
may retune `a76-pi5` on suspected proxy misrank, but they never
gate landing and are never a `check` column. Semantics must not depend
on which mode is selected; both modes stay correct under the ordinary
oracles.

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

### 5.1 Pixels compiler data and one-ISA obligations

Pixels adds `FieldGraph`, `MaterialGraph`, and `FrameProgram` as
compiler-owned data, not executable IR. Source and generated runtime functions
still lower through the existing FlowWir → MachineWir → AArch64 path.
`FieldGraph` is an ephemeral canonical graph used while compiling `@field` and
`@material`; `FrameProgram v1` is immutable image data consumed by the
generated standard-library renderer. Neither may add executable semantics that
bypass the ordinary lowering pipeline. The full contract is
[07 §1.1](07-pixels.md#11-compiler-data-not-another-executable-ir).

Machine v1 is a one-ISA target: every emitted hot workload carries a
machine-readable obligation listing the source operation, selected AArch64
instruction or fixed sequence, feature requirement, register/stack/call
constraints, and cost rows. Compiler conformance proves that every obligation
has exactly one legal selection and that every emitted hot instruction is
covered. There is no runtime dispatch or scalar portability fallback inside a
sealed hot workload. `FEAT_DotProd` is not part of the current baseline; Pixels
may require it only after the planned P12 additive machine-v1 baseline update
and matching host conformance gate.

## 6. The image report

Every successful build emits a machine-readable report and a summary. At
minimum: build identity (compiler, revision, target, the build-affecting
constants — quotas, thresholds — and digests of every input); memory by owner and site with peak ceiling; every promotion
with its why-chain; per-actor mailbox logical capacity and physical bytes;
frame slot counts, sizes, and overlays; stack bounds (executor, ISR, fault);
pool capacities (image and scoped) with reclaim destinations; the image
failure policy;
queue shapes and maximum in-flight operations; every logical actor edge and
its physical lowering (queued / direct / forwarded / fused); checkpoint
sites and proven elisions; maximum interrupt-masked interval; receipt
handoff edges with their recovery nodes; every data copy above the
reporting threshold, with site and size; a short proxy-cycle summary
(model version, digest, schedule totals by owner — not per-Term lines;
those live on `wrela dump --stage=cost`) plus one **per-core text and
translation budget** line per core, giving that core's hot text against
its 64 KiB L1I and its page span against the 48-entry I-TLB and the
1280-entry L2 TLB, since [§5](#5) makes that the hard constraint on code
growth and the budget is only meaningful per core; baked artifact hashes;
and code and data size by owner. Expected device exit rates remain a
separate report intention ([06 §5](06-machine.md)), not this summary.

Tooling (hover, expanded views) must display inferred facts wherever source
omits them: receiver effects, pool names, reclaim classification, the
computed per-type classes (`copy`, `must_consume`, `crosses_actor`,
`holds_authority`), copy vs move at each binding, actor edge vs internal
call, suspension points and frame fields, the ambient group lineage of
every `async fn`, the inferred requirement set of every generic, the
inferred error set of every private `-> Result[T]` ([02 §5](02-language.md)),
which driver methods carry the handoff convention, and DMA state around
protocol calls. Inference reduces annotations; it must not hide causality.

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

## 7. Reproducibility and phases

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

## 8. Boot

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

## 9. Record/replay

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

## 10. Conformance

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
