# Roadmap: the dumb-and-correct build

Dumbness is a **permanent guiding principle**, not a v0 tactic. Work speed
and correctness are the goals; runtime speed is purchased later, and only
with evidence. Two structural facts make this safe here when it is risky
everywhere else:

- the as-if architecture (WIRs, verifiers, the report —
  [04](docs/language/04-compiler.md)) means every naive choice can be
  replaced by a provably equivalent one — dumb code is not debt, it is a
  reference implementation waiting to be beaten; and
- deterministic record/replay makes profiling exact: one recorded workload
  replays identically under instrumentation, so attribution is
  cycle-precise, regressions bisect like correctness bugs, and the
  compiler's *predicted* costs (budgets, copy prices, exit rates) can be
  diffed against *measured* ones — the profile is itself an oracle on the
  cost model.

## Doctrine (decided once; sessions do not relitigate)

- Compiler: single-threaded batch pipeline. No query system, no incremental
  compilation, no interning, no arenas. Clone freely. Hand-written
  recursive-descent parser. Sema as multiple whole-tree passes.
- Determinism through dumbness: `BTreeMap`/sorted `Vec` on every
  output-touching path, never `HashMap` iteration; no threads; no wall
  clock. Reproducibility passes by construction.
- Evaluator before backend: the tree-walking comptime evaluator is the
  reference implementation of the semantics. No bytecode.
- Backend: embarrassingly naive. Fixed frames, spill everything, every
  check emitted, identity FlowWir "optimization". `diff-eval` makes it
  trustworthy.
- VMM: no QEMU. The machine has no GIC, so QEMU's `virt` board is a bad
  fit; a minimal wrela VMM (load blob, run vCPUs, PV pages, console +
  clock) is small and is the runner we keep.
- Diagnostics are the one place not to be dumb: errors are pinned golden
  artifacts and a core feature.
- **No foreign code in an image, ever.** 01 §2 already forbids the
  mechanisms ("There is no dynamic loader, JIT, runtime dispatch, `dyn`
  type, or unbounded task creation"); this bullet names the consequence,
  because the temptation recurs in one specific shape — importing a
  general-purpose OS's driver stack instead of writing drivers. Both forms
  are permanently out. *JIT-ing driver code at boot* deletes sealed images,
  digest validation, whole-program reachability,
  `compiler.repro.byte-identical`, and record/replay in a single move
  (JIT'd code is not in the recording). *Fusing a Linux driver at comptime*
  is self-defeating rather than merely hard: such a driver is a client of
  kmalloc, the DMA API, workqueues, RCU, raw aliasing, and inline asm —
  exactly the constructs this language exists to forbid — so importing one
  turns every guarantee in the report (memory ceiling, DMA ownership,
  checkpoint bounds, no dynamic allocation) into a false statement, and
  GPLv2 makes every image a derivative work besides. The legitimate form of
  "reuse Linux's driver knowledge" is to read a driver as a *specification*
  of the hardware's register contract and then write the driver in wrela
  under 03's rules: documentation that happens to be C, not fusion.

## The session loop

Pick one ledger gap → write the expected output first, from the docs →
implement the dumb version → `cargo xtask check` → flip the clause, cite
its id in the commit → commit. A session that cannot reach green ends with
`git restore`, not a "mostly done" tree.

## Milestones

Done means: goldens pinned, ledger clauses flipped, `cargo xtask check`
green. Each milestone names the clauses it flips (existing ids) or opens
(new ids added in the same commit as the work).

**Planning is two-resolution.** This file stays coarse for everything
beyond the active milestone. A milestone's *first deliverable* is its plan
— `plans/M<n>.md`: the ordered clause walk, the golden cases named before
code exists, the shape decisions that milestone freezes, and explicit
non-goals. Plans are written when a milestone activates, never earlier
(each milestone manufactures the facts the next plan needs), and become
history when it completes. The active plan: [plans/M9.md](plans/M9.md).

### M1 — Parse everything
Full grammar → stable AST dumps (`wrela dump --stage=ast`). Includes
finishing and hardening the lexer (floats, escapes, doc comments) and
deterministic in-tree fuzzing of both lexer and parser — no panics on any
input, every find committed as a golden error case — plus a
pretty-print/reparse roundtrip oracle. Detail: [plans/M1.md](plans/M1.md).
The spec corpus (`cargo xtask corpus`) is the test suite: every ```wrela
block in docs/language/ must lex and — except `...` fragments — parse.
The **compiler lane of `bench` comes alive here**: `wrela --timings`
reports per-phase wall time and peak RSS (trivial in a batch pipeline —
each phase boundary is a timestamp), and `xtask bench compiler` times the
pipeline over the corpus and the virtio example, then locks a threshold so
compile speed cannot creep. Compile time is a product number, not dev
comfort: sealed images mean every app update is a full rebuild, and
`xtask check` latency is the inner loop of all agent work.
Flips: `docs.examples.wrela-blocks-lex`. Opens: `syntax.*` clauses per
grammar rule as they land.

### M2 — Reject everything wrong
Name resolution, types, access modes, resource moves, definite
initialization, structural generics — as a diagnostics golden corpus.
Checking before running: this is where the language's guarantees live.
Flips: `source.identifiers.ascii-only`, `values.data.copies-implicitly`,
`values.resource.move-spells-take`, `values.exclusivity.no-overlap`.

### M3 — Run pure code
Tree-walking evaluator, comptime legality checking, `comptime assert`,
`@test` comptime tier. The language is alive with zero emitted
instructions. Opens: `comptime.*` clauses.

### M4 — Build images on paper
`@image` evaluation, graph checks (wiring, pools, supervision, restart
provisions), and the image report as a golden artifact. The virtio example
"compiles" to a diffable report before any runtime exists. Opens:
`image.*` clauses.

Settled (2026-07, human decision): `group` and scoped `pool` stay two
constructs. They share one region model but differ where it matters —
pools are nominal and compile-time (`own[P] T` in signatures), groups
are anonymous and inferred; groups join the wait-for graph at close,
pools never wait; DMA reclaim is gated on quiescence, not scope exit.
Unifying the surface would either drag pools into the progress analysis
or hide two semantics behind one keyword. The relationship is now
stated normatively in 04 §3; M4's `img.pool` goldens pin the
two-construct vocabulary deliberately, not by default.

### M5 — First boot
Naive A76 backend for synchronous code, minimal runtime, minimal VMM
(console + clock, hvf first) → a wrela program prints over virtio-console
on a Mac; pinned as a boot golden. `repro` and `diff-eval` stop failing
closed here — and so do `profile` and `bench`: record a workload once,
replay it under counters, and diff the report's predicted costs against
measured ones. Measurement is a deliverable, because the cleverness budget
(below) cannot be spent without it. Flips:
`compiler.repro.byte-identical`, `compiler.eval.matches-backend`,
`machine.boot.no-discovery`. Opens: `compiler.costs.predicted-vs-measured`.

### M6 — Actors on one core
Turns, mailboxes, `send`, groups, deadlines, cancellation, checkpoint
injection — core 0 only. Record/replay comes alive (checkpoint injection
is the only delivery mechanism, so determinism is free). Flips:
`actors.turns.non-reentrant`, `actors.send.statement-requires-proof`,
`machine.interrupts.checkpoint-injection`.

Design constraint recorded now so the recorder doesn't foreclose it: the
machine's only scheduling nondeterminism is cross-core admission order
plus device completion timing, all injected at checkpoints (06 §8) — so
for a small scenario the schedule space is finite and *enumerable*, and
a later milestone can systematically explore admission orders and
completion timings and assert invariants under all of them: model
checking of the actual image, no bespoke simulator. The dumb version is
the right version — exhaustive enumeration under a budget, fail closed
when the space exceeds it; no partial-order reduction without a profile.
What M6/M8 must preserve: the recorder's schedule representation is an
enumerable choice sequence, not just a replayable log.

### M7 — A real device
virtio-blk model in the VMM + the stdlib driver: capabilities, queues,
receipts, DMA ownership, reset — chapter 03 end to end on one device.
Opens: `hardware.*` clauses.

### M8 — Multicore
Placement inference, cross-core rings, **3 vCPUs**, per-mailbox admission
recording. Flips: `actors.placement.deterministic`.

Settled (2026-07-24, human decision): the machine is **3 vCPUs, always** —
down from 4. The flagship host runs a thin Linux underneath the VMM (core
isolation plus VFIO for devices), and one core is pinned to it permanently:
Linux housekeeping (timers, RCU, unoffloadable kthreads), the VMM's polling
I/O threads, and the recorder. The trade is deliberate — one core buys the
entire device driver stack of a general-purpose OS, against a machine whose
wins are in the overheads that surround compute (no syscalls, no context
switches, no scheduler jitter, no allocator) rather than in ALU throughput,
which is exactly where a message-passing machine collects them.

Record the **derivation, not just the number**, so the next board change is
arithmetic instead of rediscovery: `vCPUs = flagship core count − 1
housekeeping core`. Today that is 4 − 1 = 3 on a Raspberry Pi 5. It is a
machine-revision constant; a different flagship recomputes it deliberately,
in its own commit.

Rejected alternative (recorded once): keeping 4 vCPUs and moving the
flagship to an 8-core RK3588-class board, where the A76 cluster would be
the machine and the A55 cluster the housekeeping — a good fit on paper
(the ISA baseline and `wrela-cost-v1` are both A76-defined) and rejected
because the board is a product decision, not a contract-driven one.

The blast radius is small and belongs to this milestone's own plan, not to
an ad-hoc edit: 06 §1's "4 vCPUs, always" and README's summary line (both
normative — minimal edit, same-commit clause, REVIEW-QUEUE line);
`wrela-machine`'s stack-region doc comments; and the four `core_stack_N`
report entries, which become three and move every golden carrying a layout
section. Because the report enumerates cores, this is a golden-moving
change and must land as its own reviewed commit.

Note for the scaling question this decision raises: **Linux's own
housekeeping never needs more than one core at any scale** — it is constant,
not proportional to I/O. What scales with I/O is the VMM's own
virtio→hardware translation (~2M descriptors/sec/core; roughly one PCIe 4.0
x4 NVMe, or 10–40 Gbps of networking, per core). That cost exists only
because the guest speaks virtio while the device speaks NVMe, and it goes to
**zero** — at any I/O rate, with the virtio contract intact — on vDPA
hardware, where the guest's queues are the device's queues and the VMM
leaves the data path entirely. Recorded so a future bandwidth problem is
answered by hardware selection rather than by conceding cores.

### M9 — The stdlib
The milestone the code has been citing by name since M2 without it ever
existing on this ladder (`ledger.toml`, plans/M2–M5 — grep "stdlib
milestone"). Today `stdlib/` is a README and an empty directory, while
`sema/prelude.rs` — 84 hardcoded lines whose own doc comment calls itself
a placeholder — stands in for the real surface.

Lands: the `Format` contract and f-strings (dead in sema since M2 —
`sema::bodies` returns `error[unimplemented]: f-strings are not checked
yet` at three sites), `Result`/`from` conversion, the collection/format/
time surface of [05](docs/language/05-library.md), and the `ImageReport`
reflection type `@layout_assert` needs to run at all.

Two structural consequences worth more than the features: `sema/prelude.rs`
is **deleted**, and the doc corpus becomes sema-checkable. `xtask corpus`
stops at parse today precisely because the docs name stdlib types
(plans/M2.md decision 5) — so the normative chapters are currently lexer
inputs, not type-checked ones, and closing that makes ground-truth rule 1
mechanical rather than aspirational.

Flips: `comptime.fstring.bounds`, `image.report.layout-asserts`,
`values.resource.protocol-consumption`. Unblocks the inferred-error-sets
intention below (still human-gated).

### M10 — The runtime in wrela
Today the runtime is not a library: it is hand-assembled A64 emitted word
by word from `layout.rs` (`build_rt_select_and_run`, `build_rt_enqueue`,
`build_rt_run_one`, `build_group_child_poll`, the console formatter
`build_fmt_dec`/`build_ring_append`, the abort stubs), with branch offsets
patched by index. It is the one module that fails the regeneration test,
and it is exempt from every oracle this project owns — no stage dump, no
sema, no FlowWir, no `diff-eval`, no mwir golden. Its only oracle is a boot
transcript; its one known bug so far (a missing `x30` save,
`build_rt_select_and_run_core`'s own comment) was found by a test hanging
forever.

It is also the hottest code in every image — the scheduler runs between
every turn — and therefore the one place the cleverness budget
*structurally* cannot be spent: no profile, before/after, or lock reaches
it. Hand-assembly is not fast. It is **unimprovable**, which is worse.

So the scheduler, mailbox admission, turn execution, group child polling,
the console formatter, and the abort path become **wrela source**,
compiled by the same backend as user code, dumped at every stage, and
pinned by the same goldens. The image stops having a Rust-shaped hole in
the middle of its "one designed machine, front to back" claim.

**The floor (settled here; the "what must be Rust" question, answered
once).** The compiler and the VMM are Rust permanently — the VMM
*implements* the machine, it is not part of it. Guest-side, exactly three
things cannot be compiled code, totalling roughly twenty instructions:
code running before SP is installed (every prologue assumes a stack);
code that must clobber no register (the checkpoint stub's save/restore);
and instructions with no expression form (`eret`, `brk`, barriers,
`msr`/`mrs` — note `encode.rs` today has only `enc_brk`, which is the
measure of how little the machine needs). Those stay hand-encoded in
`layout.rs`, pinned as a byte golden, and never grow.

**The one rule that makes the rest fall out.** *Every runtime-varying
reference in the machine is an index into a statically-sized, uniformly-
strided array — no exceptions.* The machine already obeys this everywhere
but one place: method dispatch is an index into a build-time table, the
round-robin cursor is an index, the ready queue is `actor_count + 1`
slots, the group arena is sized from a static count of `with group` sites.
The single anomaly is the **waker**, which is a raw address into some other
actor's turn area — and it is an address for exactly one reason: in
hand-written assembly an address is a cheaper instruction than an index.
That is an artifact of the medium, not a design decision.

Conforming the waker costs one thing: turn areas are variable-size today
(`ActorRuntimeLayout::frame_size` — record plus that actor's widest async
frame). Make them **uniform**, sized to the image-wide max and rounded to a
power of two. You spend padding bytes and you buy an array type; index →
address becomes a single shifted-register add, and the report already
publishes peak memory so the cost is visible and locked rather than hidden.
Spending bytes to buy a type is the trade this project already makes with
fixed frames and spill-everything.

With that rule in force the whole `rtdata` section is one typed static:

```text
@layout(runtime) struct RuntimeTables {
    turns:     [TurnArea; N_TURNS],     // uniform stride, TurnId indexes it
    mailboxes: [Mailbox;  N_ACTORS],
    groups:    [GroupSlot; GROUP_CAP],
    rr_cursor: usize,
}
@placed(RTDATA_BASE) static RT: RuntimeTables;
```

**One intrinsic, one use site, in the entire system** — and `@layout(runtime)`
is not a new mechanism but a fourth layout class beside M7's
`@layout(mmio|dma|wire)`, so exact offsets/padding/sizes land in the report
through machinery that already exists. `verify_section_sizes` already knows
how to check the placed size against `compute_runtime_tables`. A `TurnId`
newtype over an index makes a waker a *value* sema can reason about, whose
only legal use is indexing `RT.turns` — which also sets up bounds-check
elision later as a **proof** (the same shape as the existing erasure of
impossible `CallError` variants, 02 §9.4) rather than as a fast path. Do
not do that elision in M10; only make it possible.

Intrinsic surface, target: **`@placed`. One.** It is barely new —
`wrela-machine` already defines fixed addresses; this is the language
naming one. The normative doc change and ledger clause come **first, in
their own commit, before any migration** — both because CLAUDE.md's house
rule requires it, and because migrating routine-by-routine and adding
whatever intrinsic each one turns out to need designs the permanent surface
by accretion, in the one place this project can least afford it.

**`@brk` is deliberately *not* on that list**, and the reason generalizes.
Today every `BRK` in an image is compiler-internal: no wrela source spells
one, no author can opt out of one, and the only way an author meets one is
as a VMM diagnostic line after something already broke
(`decode_brk`/"unexpected `BRK #imm`"). Making it spellable would trade
that property away and hand user code a trap-with-no-diagnostic competing
with `panic`. It is also unnecessary, because typed wrela dissolves the
sites rather than translating them:
`BRK_ASYNC_DISPATCH_NO_STATE_MATCHED` is emitted by codegen itself at every
resume dispatch tail, so the runtime inherits it free;
`BRK_LINE_APPEND_OVERFLOW`/`BRK_LINE_COMMIT_OVERFLOW` become ordinary
array-bounds checks on the fixed-size console ring, which codegen already
emits everywhere — better than today, since a bounds failure gains a
diagnostic instead of a bare trap; and `BRK_REPLY_SLOT_NO_WAKER` becomes
*unrepresentable* once the waker is a non-optional `TurnId` field. The two
that resist — `BRK_ACTOR_TURN_CANCELLED`/`BRK_AWAIT_ACTOR_REJECTED` — are
not unreachability guards at all but a **representation gap**, named as
such in their own comment ("the turn record carries one scalar reply word
and no error tag, so there is nothing to hand the awaiting turn but a
lie"). So the standing rule for this milestone: **a surviving explicit trap
is a finding about the representation, not a case for an escape hatch** —
it says the reply channel needs a tag, which that code already predicts.
`@brk` may be added only per-site, with that argument made and rejected in
writing first. The floor keeps `BRK` regardless (the halt tail); nothing
above the floor spells it.

**Rejected: `@naked` + inline asm.** It is a real language feature with real
semantic weight (no prologue, no stack, register discipline sema cannot
verify) — a large surface to add to a language whose premise is that it
checks everything, bought only to relocate twenty reviewable, byte-pinned
words. It also requires an assembler (mnemonics, operands, labels,
relocation) — plausibly `encode.rs`-sized plus a parser, permanently, with
its own goldens and fuzz lane. And it is a general escape hatch in a
language that deliberately has none: today an inexpressible thing is a
fail-closed error with a named gap, and that pressure is what has been
improving the language every milestone. The asymmetry decides it — twenty
hand-encoded words cannot grow, because nothing in the language can reach
them; `@naked` grows by default. It can be added later on evidence M10
itself would produce. It cannot be removed later.

**Migration discipline (what makes this safe at all).** Item order, and
item 0 matters most:

0. **Uniform turn areas + `TurnId` instead of waker addresses — still
   hand-assembled.** A pure representation refactor in Rust, verified
   byte-identical on every boot golden, landing before any language change
   is in flight. This is the highest-value split in the milestone: it means
   the wrela migration afterwards is a **pure translation with no design
   decisions left in it**. One variable at a time; two easy steps instead
   of one hard one.
1. `@layout(runtime)` + `@placed` as the doc/ledger commit (above).
2. Console formatter — touches no runtime table at all, pure computation
   over a buffer. Proves the toolchain end to end at zero risk.
3. Abort path — the *body* only (print `x0..x5`'s message over the ring);
   the halt tail (exit code, `EXIT_MMIO_ADDR`, `BRK #0`) stays floor. Still
   no tables.
4. `rt_enqueue` — first table access, one mailbox, no dispatch.
5. `rt_run_one` / group child poll.
6. `rt_select_and_run` last: most complex, most evidence available by then.

Each routine's wrela version must produce a **byte-identical boot
transcript** against every existing boot/replay golden *before* its
hand-assembled version is deleted — the transcripts pinned by M5–M9 are the
differential oracle, and the hand-asm implementation is the reference the
new one is diffed against, exactly as `diff-eval` uses the evaluator
against the backend.

**What M10 costs, stated honestly up front.** It will probably make the
scheduler *slower*: bounds checks on every index (the hand-asm has none)
and the spill-everything frame convention (the current scheduler keeps
everything in registers across its whole body). Some of that is offset —
the current mailbox slot computation uses a `mul`, which power-of-two
striding turns into a shifted add — but assume a regression and do not
pre-optimize to avoid it. **That is the trade, and it is the right one:**
because the transcripts are byte-identical, `bench guest` measures the
identical workload before and after, so M10 hands you an exact before/after
on an identical recording — precisely the evidence the cleverness budget
demands and which has never been obtainable for this code. Lock the bench
before migrating, measure after, and record the delta in the plan as a
known number.

**Two things to verify before this becomes plan text** (neither is
established): that uniform turn areas are actually cheap for real images —
`compute_runtime_tables` already knows every frame size, so a short
measurement over the existing goldens settles it, and if the padding
multiplies badly the fallback is a `[u64; N_TURNS]` offset table (one extra
load, back to today's cost, still an index and never an address); and that
nothing else in the runtime holds a raw address — the group arena and the
checkpoint stub's saved-register area have not been traced closely.

Take the free structural win while in there: `layout.rs` is 6339 lines
doing five unrelated jobs — section packing, relocation resolution, report
rendering, runtime codegen, and the boot harness. "Prefer long obvious
files" is a good rule that this file has outgrown; the runtime routines in
particular have nothing to do with section packing. Extracting them is
already implied by migrating them, so the split costs nothing extra here
and should not be deferred to a separate cleanup that never happens.

Opens: `runtime.*` clauses (there are none today — every one is opened
here). Non-goals: self-hosting the compiler; touching codegen; and
optimizing the scheduler — M10 makes the scheduler *reachable* by the
cleverness budget, it does not spend it. The first optimization pays the
full three-part price like everything else.

**Settled here (two rejected alternatives; do not relitigate).**

- **A single fused "global state machine" for the whole image's async
  structure.** Rejected as the wrong first move, on three grounds. (a) The
  real per-turn cost is two linear scans, not the dispatch mechanism:
  selection is O(actors) (`build_rt_select_and_run`; the ready-queue table
  is already sized and placed and deliberately unpopulated —
  `RuntimeTables::ready_queue_capacity`'s own doc comment) and dispatch is
  an O(methods) compare chain (`build_rt_select_and_run_core`'s
  `methods.iter().enumerate()` loop). Fusion buys neither — a fused machine
  still has to *pick* which actor runs. (b) One giant switch is a single
  polymorphic indirect branch with N targets — the classic interpreter
  dispatch problem, whose usual remedy is itself heavy cleverness; N
  distinct `BL` sites predict individually. "Fuse to go faster" is a
  hypothesis that could measure negative on an A76. (c) It collapses N
  per-function FlowWir dumps and goldens into one enormous dump, trading
  the primary review surface for an unmeasured win. Note the dispatch chain
  exists *because of the medium*: a dense match over a comptime-known index
  is a jump table, and nobody hand-patches a jump table by index — so
  migrating to wrela relocates that fix to one codegen improvement
  benefiting every `match` in the language. And after M10 fusion is a
  *lowering* decision with `diff-eval` as its oracle, not a rewrite —
  another reason it comes after, never instead.
- **`WFE`/exclusive-monitor "yield on a memory address" as the idle or
  wake mechanism.** Rejected, and largely already decided. The instinct is
  correct and is *already the architecture* one layer up (06 §5: a
  shared-memory doorbell word per queue plus one host-visible wake; park
  writes its next deadline and the VMM sleeps the vCPU thread) —
  implemented where the recorder can see it. `wrela-machine`'s park
  doorbell already records the rationale for a trapping store over a WFI
  trap. Three further reasons it stays rejected: `WFET`/`WFIT` (wait *with
  timeout*, which a deadline park needs) is ARMv8.7-A while this machine's
  baseline is **ARMv8.2-A** (06 §1), and bounding plain `WFE` with the
  generic-timer event stream would make a deliberately tickless design
  tick; `HCR_EL2.TWE` means WFE traps under both KVM and HVF anyway, so the
  imagined in-guest sleep is an exit either way, and a trapping store reuses
  the `decode_data_abort` the VMM already has. Decisively: a core woken by
  another core's `STXR` clearing its exclusive monitor is a hardware event
  the VMM cannot observe — not recordable, not injectable — which would
  blow a hole through record/replay, the enumerable choice sequence, and
  all of the DST work that rests on them. The one nuance worth preserving:
  WFE wakeups are architecturally permitted to be spurious, so a park loop
  that re-derives readiness from memory is idempotent and the wake decides
  nothing. That leaves WFE admissible *later* as a pure power optimization
  underneath an already-recorded park/unpark decision — a cleverness-budget
  purchase against idle cores, which do not exist before M8.

### M11 — The cost contract
Perf without chasing hardware. Today `compiler.costs.predicted-vs-measured`
is a gap whose own note says it plainly: `report::render` predicts no costs
anywhere, so `profile` has nothing to diff its measurements against. This
milestone builds the prediction side, and it builds it as a **contract**,
not as a chip.

**`wrela-cost-v1` — the cost model is designed, not inherited.** 06 §1
currently says "the compiler's one cost model is the A76," which is the
last place this machine inherits rather than designs. The ISA line one
sentence earlier already shows the right pattern — ARMv8.2-A, "the
intersection of Cortex-A76 and Apple Silicon," a contract real chips
happen to satisfy. Do the same for cost: a versioned parameter file beside
`wrela-machine-v1`'s address constants, holding the latency table, issue
width, port set, ROB depth, cache geometry, and branch costs as **data** —
checked in, diffable, revised only in its own commit
(`bench/thresholds.toml`'s precedent exactly). The A76 is demoted from
*definition* to **calibration donor**: the chip v1's constants were first
measured on, and the floor the envelope tracks. The unit is the **v1
work-cycle**, matching 02 §883's own word for `@budget` ("proven work
bound") — defined by the contract, measured in the world.

Three consequences, each solving a problem that would otherwise need its
own mechanism:

- **Soundness gets a direction.** The profile is the *pessimal envelope*:
  every conforming host must meet or beat it — the performance analogue of
  the ISA intersection. A `@budget(cycles=N)` proof discharged against the
  profile is then sound on every host, and `predicted-vs-measured` stops
  being a vague "diff them" and becomes an inequality with teeth:
  **measured ≤ predicted, always.** A host that exceeds the envelope is
  either a model bug or a nonconforming host — both findings, both
  pinnable. That is also the crisp definition of "supported host" this
  project lacks: ISA conformance asserted at boot, cost conformance
  asserted at calibration.
- **Determinism comes free.** The model is a pure function of (instruction
  stream, profile file), so cycle goldens are byte-identical on every dev
  machine regardless of the chip in it. That property is what makes cycle
  counts *goldens* at all; a model defined as "the A76" would leak
  whichever laptop measured it.
- **Revision already has a ritual.** Changing a constant moves goldens, so
  it goes through `golden --update`: deliberate commit, diff reviewed,
  clause cited. And 04 §7 already requires build identity to carry "the
  build-affecting constants" — the profile version slots into an existing
  report line.

**Two models, two soundness requirements.** *Static, worst-case, per turn*:
data-dependent branches take their worst outcome, loops take the bounds
`sema.bounds.loops` proves; must be a sound **upper bound**, because this
is what discharges `@budget(cycles=N)` — an optimistic proof is a lie — and
fails closed when it cannot bound something. *Replay-exact, per recording*:
deterministic replay fixes every branch outcome and every address, so the
only residual uncertainty is microarchitectural, never semantic; this one
scores optimization search and backs the cycle goldens.

**Build order, largest error term first, each independently verifiable.**
(1) Structural simulation — a scoreboard of per-register ready cycles,
per-port free cycles, and a decode/ROB cap, walked over the emitted
instruction stream. Not "simulate an A76" (unverifiable) but model the
declared resources: ~200 lines, no recursion, deterministic, and the
difference between ~3x error and single digits. Note this is only possible
because there is no LLVM — `codegen.rs` emits every word, so the model
reads the exact final stream with no black box between. (2) Exact cache
simulation from the replay address trace — static layout means the address
sequence is known, so misses are computed, not estimated. (3) The branch
taxonomy: bounds checks, checkpoint tests, and overflow checks are
statically biased and predict near-perfectly; a loop with a proven trip
count mispredicts exactly once per execution; only genuine data-dependent
conditionals are unmodelable, and the report **declares** them rather than
folding them in — `812 cycles (780 exact, 32 estimated across 2 sites)`.
(4) The prefetcher, only if (2)'s residual says it matters.

**Zero tolerance on the semantic half.** The replay-exact model also
predicts every *architectural* count the recorder already logs — vCPU
exits, clock reads, transcript bytes, checkpoint crossings (`xtask profile`
prints these today; `bench guest` already asserts them identical across
boots). Those must match **exactly**: they are semantic, not
microarchitectural, so a mismatch is a bug, full stop. Timing error is a
calibration question; semantic error never is. The whole semantic half can
be built and verified before a single latency constant exists.

**Cost carries why-chains.** 04 §7 already requires whole-image analyses to
show causality ("Inference reduces annotations; it must not hide
causality"). Apply it to cost: every term cites its site and the profile
constant that produced it, so a golden diff of `cycles_per_turn: 812 -> 852`
expands into which block, which instruction class, which constant. That is
what makes a cycle regression reviewable like a correctness change instead
of a number that moved — and it is the audit trail that makes a
search-found "win" show its work.

**Calibration is a pinning discipline, not an afternoon on a laptop.**
Never model an effect without an isolating microbenchmark that pins it;
each modeled element (port pressure, ROB depth, L1 geometry, prefetcher)
gets one, measured once on real hardware and committed as a constant. When
measured deviates from predicted beyond tolerance, handle it exactly like a
fuzz find: minimize to an isolating case, pin it, re-lock the constant it
exposes — never nudge a number until the diff goes quiet. Calibration
workloads are themselves recorded, replayable, in-tree goldens. This gives
real hardware its correct and minimal role: **calibrating parameters,
rarely — never chasing regressions.**

**Acceptance rule (settled here).** There is **no percent threshold on a
win**, because the reason thresholds exist — measurement noise — does not
apply to a pure function. The model has zero variance and nonzero *bias*,
so the gate is the model's own declared uncertainty on the terms a change
touches: a 1-cycle win in exactly-modeled code is a real cycle and lands; a
50-cycle win concentrated in data-dependent estimates is untrustworthy at
any size. The rule self-tightens as calibration improves, with no constant
to relitigate. Directions are asymmetric: **regressions threshold at zero**
— any increase is a golden diff that must be explained — while wins have no
floor. And the complexity gate stays where it already is: a small win that
adds a special case fails the regeneration test, and would fail it at 100
cycles too. Never reject a win for being small; only reject complexity that
is not paid for.

**The model proposes; the recording disposes.** Search may use the model to
rank a million candidates — that is what it is for. **Landing** still pays
the cleverness budget's full three-part price, including a before/after on
a named recording. The model never becomes the landing authority. That is
the structural anti-Goodhart bound: exploiting a model bias can win a
search, but it cannot merge, because the recording will not corroborate it.

Flips: `compiler.costs.predicted-vs-measured`. Depends on
`sema.bounds.loops` (whose own gap note already says proving it "needs the
comptime engine and cost model" — the two are mutually referencing, and
either half helps the other). Requires a normative edit to 06 §1's
cost-model sentence: minimal, same-commit clause, REVIEW-QUEUE line.
Non-goals: optimizing anything (M11 builds the oracle; spending it is the
cleverness budget's job, in the order recorded below); multicore
contention modelling (single-core per-turn is the granularity optimization
decisions are made at, and cross-core sharing is a measured calibration
factor, not a modelled one); DVFS and thermal, permanently out of scope.

**Settled here: no learned policy inside the compiler.** A fast
deterministic oracle is exactly what learned compiler optimization needs,
and it demonstrably works elsewhere — that is not the objection. The
objections are that a weights blob is the most anti-regeneration artifact
that exists ("any crate should be rewritable from docs + contracts + tests
alone" — a learned policy is definitionally not), that 04 §7's
must-not-hide-causality requirement is violated by construction (a policy
cannot answer "why did it spill x9?"), and that Goodhart goes from
incidental to systematic and uninspectable, since the policy is *trained*
to exploit model bias. The win is available without the artifact: run the
search offline and ship a **table**. Bounded exhaustive search under a
budget, failing closed over it (structurally identical to the DST schedule
enumeration in the coverage pass); superoptimization over small windows
whose verified result commits as a peephole table — data, diffable,
regenerable from oracle plus search. Register allocation needs no learning
at all: linear scan, the scorer, and bounded search over spill choices. The
rule, stated once: **ML may inform an artifact; it may never be an
artifact** — the same relationship this project already has with fuzzing,
where the fuzzer is not in the compiler and the pinned case it found is.

### Recorded language intentions (not yet scheduled)

- **Inferred error sets** (stdlib milestone, via doc revision): extend
  "pub declares, private infers" — the doctrine receiver effects, pool
  names, generic contracts, and comptime legality already follow — to
  error types. A private `fn` omits its error type; the compiler infers
  the exact set from the closed world (it already computes this to erase
  impossible `CallError` variants, 02 §9.4); `pub` boundaries still
  demand a declared nominal enum. Lands when `Result`/`from` conversion
  machinery is real; it is a normative doc change first, human-reviewed.
- **Deferred until an ingredient exists**: an end-to-end latency
  assertion (`@latency_assert`) waits for the measured cost model
  (`compiler.costs.predicted-vs-measured`); graph-level flow policy
  waits until a concrete image needs a concrete named check — hardcode
  that check, no policy query language; whole-image snapshot/time-travel
  is a VMM feature to weigh after M6's recorder exists.
- **The flagship host** (recorded 2026-07-24; deliberately *not* a
  milestone — human-gated like everything else in this section). CLAUDE.md
  names the flagship as wrela OS on Raspberry Pi 5 / 1 GiB, and
  [06 §](docs/language/06-machine.md) names Linux/KVM as the product
  backend — but `wrela-vmm`'s `kvm` module is unimplemented and every
  hardware-facing path is `#[cfg(all(target_os = "macos", target_arch =
  "aarch64"))]`. M5–M11 all boot on Hypervisor.framework on a Mac, and
  `xtask check`'s boot/repro/diff-eval/bench-guest lanes fail honestly
  (never silently skip) on any other host. So the ladder's development
  host is not the product's host. Recorded as a known, deliberate gap so
  it is a decision rather than an oversight; scheduling it is a human
  call. Shape decided 2026-07-24 (see M8): the flagship runs a **thin
  Linux under the VMM** — core isolation, VFIO passthrough for devices,
  one core pinned to housekeeping — never bare metal. wrela owns three
  cores and every device contract; Linux is a bootloader, an IOMMU
  configurator, and a janitor. The alternative (bare-metal Pi: PCIe
  bringup, real-hardware drivers, and a machine revision replacing 06 §6's
  virtio-family device set) is rejected as a driver-engineering project,
  not a language project. Note the VMM boundary is also what makes every
  device interaction a recordable choice point, so this is not a
  concession — record in the field, replay in the lab, one image.

  *How real hardware gets behind a virtio contract, without the guest ever
  learning about it.* 06 §6's device set is closed and virtio-family, and
  06 §3 already says "device topology is a **build output**, not a probed
  fact" — so the driver/device seam is the insulation layer, and how the
  VMM *backs* a contract is its own business. Three backings, in increasing
  order of how much of the host they erase: a file or ramdisk (today);
  **VFIO passthrough**, where a userspace process gets direct MMIO and
  IOMMU-mapped DMA to a real PCIe device with no kernel driver in the path
  (the DPDK/SPDK architecture, mainstream and proven), with the VMM
  translating virtio descriptors to the device's own protocol; and **vDPA**
  hardware, which speaks virtio natively, so the guest's queues *are* the
  device's queues and the VMM leaves the data path entirely. The
  host-specific half of a build is legitimate and already blessed by 06 §3:
  the toolchain may enumerate the actual hardware at build time and record
  in the report which VFIO device backs which contract — comptime
  discovery, sealed into the image, guest unchanged.

  *The overhead budget, so nobody has to re-derive it.* The hot path takes
  **zero VM exits by construction** (06 §5: "Hot paths never trap"; a
  doorbell is a plain store to normal memory, a completion a plain load) —
  and a userspace MMIO exit at 2–5 µs is what kills naive VMM I/O, so the
  dominant cost is already designed out. What remains: notification
  latency (~150–300 ns, one cache-line transfer between cores),
  virtio→device translation (~500 ns, erased by vDPA), copies (zero once
  DMA lands in guest pool pages — what M7's `DmaPool` is for), and guest
  wakeup (**zero**: the event loop is already running cooperatively, so a
  completion is a memory read at the next checkpoint, not a thread wake —
  a cost that structurally does not exist rather than one optimized away).
  Realistic software round trip ~1 µs, SPDK-class. Mapping the device BAR
  into the guest's stage-2 tables as device memory lets the guest ring a
  real doorbell with no trap; combined with vDPA that removes even the
  software hop SPDK itself pays.

  *The IOMMU caveat, which changes threat model on the day passthrough
  lands.* plans/M7.md already records that the flagship host has no IOMMU
  ("pools are host-mapped directly, recorded not silently assumed"). That
  is benign **today**, because devices are emulated and the VMM is software
  that cannot scribble. It stops being benign the moment a real device is
  passed through: `vfio-noiommu` grants the physical device write access to
  all of host memory, so a device or driver fault can corrupt anything.
  Chapter 03's DMA ownership proofs are guest-side discipline and 03 §3
  already hedges ("targets with an IOMMU map only those pools") — the IOMMU
  is the hardware backstop, and the flagship has none. Recorded now so it
  is a known cost of the VFIO path, not a discovery during bring-up.

- **VMM idle policy and the power story** (recorded 2026-07-24; unscheduled,
  same human gate). The zero-exit design depends on the VMM's I/O threads
  polling doorbells, and polling burns a core continuously: order 1–1.5 W
  on an A76, against a board that idles near 2–3 W — drawn whether or not
  any I/O is happening, which would flatly contradict 06 §5's own claim
  that "idle is codesigned for power... letting the host reach deep idle
  states." The design already anticipates this in the same sentence that
  creates the problem: I/O threads "poll hot doorbells on their own host
  cores **and arm wakes when idle**." So adaptive polling is in the
  contract and the power story survives. **What is undefined is the
  policy** — how long to spin before arming, per device. That should be a
  *build output*, not a runtime heuristic: 06 §5 already has the report
  stating "expected exit rates per device," which is exactly the input, and
  the report is already the VMM's whole configuration. No general-purpose
  OS can do this, because none of them know what is coming; Linux guesses
  with `io_uring` poll timeouts and NAPI budgets. Honest crossover to keep
  in view: polling **loses** on power at low utilization and **wins** at
  high, so for a mostly-idle appliance this policy is not a detail — it is
  the entire power story. Three numbers must exist before any power claim
  is made, all of them ordinary `bench guest` measurements and all lockable
  with existing machinery: idle board power with the VMM running and no
  I/O, in both always-poll and armed-wake modes; first-I/O-after-idle
  latency in armed-wake mode (the ~5–20 µs wakeup being traded for those
  watts); and the crossover utilization at which polling becomes cheaper.
  Until those exist, "performant" is supportable and "power-efficient" is
  an overclaim.

- **Pixels** (recorded 2026-07-25; deliberately *not* a milestone —
  human-gated like everything else in this section). Display and input
  devices, a dumb scalar tile compositor, and golden frame digests were
  rung 9 of the ladder. They are now off it, and the reason is not that
  the work is hard: it is that nothing else needs it. Every remaining rung
  — the stdlib, the runtime in wrela, the cost contract — is compiler and
  machine work, and a compositor would interrupt that rather than inform
  it. Pixels is the one item whose dependencies all point *backwards* with
  nothing pointing back: the compositor is guest wrela source, so it wants
  the stdlib's closed SIMD vector set ([05 §8.1](docs/language/05-library.md),
  whose NEON lowering 04 §6 already calls a backend obligation because
  "the flagship's compositor is its hottest loop"), and its inner loop is
  a named future hot spot (see the cleverness budget), so it wants the
  cost contract — with which "tune only after a frame exists to measure"
  stops being a deferral and becomes an ordinary budget purchase priced
  against the envelope.

  *What descheduling leaves open, stated rather than implied.*
  `machine.display.golden-frames` is a gap **no rung owns**, recorded as
  such in the clause's own note — the same shape as
  `compiler.progress.wait-for-graph`. 06 §10 lists the golden-image
  display tests in the machine conformance suite, so machine v1 is not
  conformant until this lands; it keeps company there with `net`, `sound`
  and `entropy`, three more contracts in 06 §6's closed device set that no
  rung owns either. And the VMM's cross-device pool oracle stays half a
  unit test: `devices::tests::a_window_bound_to_another_device_is_refused_by_name`
  becomes a *boot* only once a second device model exists, whichever
  device that turns out to be.

  *The scope, if it is ever scheduled: **headless**.* Software scanout
  into memory and golden frame digests — never open a window, no GUI
  dependencies. That constraint was GOAL.md's standing rule while this was
  a rung and is preserved here so it is not rediscovered.

## The cleverness budget (permanent)

Cleverness is a resource, acquired only through a profile. An optimization
lands only with all three, no matter how obviously fast it is:

1. a flame graph / counter profile from a named, replayable workload;
2. a before/after measurement on that same recording; and
3. a **lock** — a bench threshold or `@budget`/layout assertion — so the
   win cannot silently regress.

Measurement has two lanes, and the budget governs both equally. The
**guest lane** — how fast wrela code runs — needs the VMM and
record/replay, so it lands at M5. The **compiler lane** — how fast wrela
code *compiles* — needs nothing but the compiler and a clock, so it lands
at M1 (`wrela --timings`, `xtask bench compiler` over the corpus, locked
thresholds). No interning, arenas, parallelism, or incrementality in the
compiler until its own bench shows the hot spot. Working hypothesis, made
falsifiable by the lock: the dumb compiler is already fast, because the
things that make compilers slow — LLVM, incremental machinery, heavy
optimization passes — are exactly the things this one does not have.

Rules that follow:

- Dumb ≠ sloppy. Fail-closed, checked-everything, and pinned diagnostics
  are correctness, not performance; they are never traded away.
- The regeneration test is the complexity budget: after any clever change,
  the module must still be rewritable from docs + contracts + tests. If a
  fast path breaks that, it first gets its own contract and verifier (the
  WIR discipline), then it may land.
- Contracts cannot be profiled into existence after the fact: checkpoint
  density, the doorbell ABI, and image/frame layout rules bake into the
  machine spec and are revised deliberately, not patched.
- Known future hot spots (compositor inner loop, naive codegen quality,
  and — once M10 lands — the scheduler, which runs between every turn and
  is unreachable by this budget until it stops being hand-assembly) wait
  their turn like everything else: the profile says when, and until then
  dumb code calling stdlib SIMD ops is the answer.
- **Where I/O effort is worth spending, and where it is not.** For
  storage, the software path is already below the device's noise floor — a
  ~1 µs round trip against a 10–80 µs NVMe read is 1–5%, so optimizing it
  further buys something invisible and spends budget that has somewhere
  better to go. For networking, where wire times are sub-microsecond,
  software dominates and the zero-exit/zero-copy/vDPA path is where the
  wins actually are. Check which regime a workload is in *before* profiling
  it, or the profile will faithfully measure something that does not
  matter.
- **The win is the tail, not the mean** — and this reframes what "beating a
  general-purpose OS" means. Throughput parity with a tuned Linux is
  achievable and unremarkable. What a general-purpose OS cannot offer is a
  flat p99.9: its tail is dominated by scheduling, interference, page
  faults, and allocator behavior, and wrela has none of those by
  construction. That win is not earned by optimization; it is already true,
  and it is the claim to defend. It also compounds with M11 — `@budget
  (cycles=N)` against the cost envelope means the tail can be **proven at
  build time** rather than measured and hoped for. No operating system can
  make that offer. Measure tails, not averages; a benchmark reporting only
  a mean is measuring the half of the story wrela does not win on.
- **The scheduler's own spend order** (recorded so it is not improvised
  the first time someone profiles a boot). Each step manufactures the
  evidence the next one needs: (1) M10 — the runtime becomes wrela, and
  the dispatch compare chain stops being hand-written by construction;
  (2) **measure** — `bench guest` over byte-identical transcripts gives the
  exact before/after that has never existed for this code, and M11's cost
  contract turns it into a zero-variance golden diff rather than a timing
  run; (3) the two
  dumb wins, if and only if the profile asks for them — populate the
  already-reserved ready-queue table (O(actors) scan → O(1) pop, no layout
  change, the slots are placed already) and lower a dense comptime-known
  `match` to a jump table (one codegen change that lifts every `match` in
  the language, not a bespoke scheduler hack); (4) only then consider
  fusion, as a FlowWir → mwir *lowering* validated by `diff-eval`, never a
  rewrite. Nothing in this list needs `WFE`, an interrupt controller, or a
  global state machine — see M10's settled rejections.

Also permanently out: abstractions serving futures that are not ledger
clauses; incremental/parallel/cached anything in the compiler until a
profile of the *compiler* demands it; second ways to do things that have
one way; "temporary" relaxations of fail-closed.
