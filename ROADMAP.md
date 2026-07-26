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
`BRK_LINE_APPEND_OVERFLOW`/`BRK_LINE_COMMIT_OVERFLOW` dissolve as **status
returns** from the console routines (plans/M10.md decision 592) — *not* as
ordinary array-bounds checks that call `__wrela_abort_val`, because that
abort path prints via the same console routines and a bounds failure
inside them would recurse unboundedly (decision 590; the abort path
carries a re-entrancy latch so a second entry skips printing and goes
straight to the halt tail — decision 591); and `BRK_REPLY_SLOT_NO_WAKER`
becomes *unrepresentable* once the waker is a non-optional `TurnId` field.
The two that resist — `BRK_ACTOR_TURN_CANCELLED`/`BRK_AWAIT_ACTOR_REJECTED`
— are not unreachability guards at all but a **representation gap**, named
as such in their own comment ("the turn record carries one scalar reply
word and no error tag, so there is nothing to hand the awaiting turn but a
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

### M11 — The cycle proxy
Perf without chasing hardware. Today `compiler.costs.predicted-vs-measured`
is a gap whose own note says it plainly: `report::render` predicts no costs
anywhere, so `profile` has nothing to put beside its measurements. This
milestone builds the missing oracle: a deterministic **proxy-cycle** score
for tuning the code wrela emits. Its question is deliberately narrower
than "how many cycles will this host take?":

> Given two semantically equivalent emitted programs, which one should be
> faster on the target?

That is the question instruction selection, spill choice, scheduling,
peepholes, bounds-check discharge, scalar replacement, inlining and every
other compiler performance pass actually need. M11 optimizes for
**differential accuracy** — ranking alternatives correctly — not for an
absolute cycle prediction and not for a portable worst-case execution-time
proof.

**`wrela-cost-v1` is a scoring contract, not a chip.** A versioned parameter
file beside `wrela-machine-v1`'s address constants holds the instruction
latencies, dependency and issue rules, branch classes, and only those
further effects calibration has earned. It is checked in, diffable, and
revised only in its own commit (`bench/thresholds.toml`'s precedent). Its
unit is the **v1 proxy-cycle**: intentionally cycle-shaped, A76-shaped by
heritage, but defined by the file rather than by whichever host measured
it. The build identity carries both its version and content digest.
Physical A/B corroboration uses the same host and harness the project
already has (`bench guest` / `profile` under Hypervisor.framework); M11
does not schedule a second calibration host.

The score is a pure function of the final laid-out instruction stream, the
profile file, and — where one exists — explicit execution weights. That
makes score goldens byte-identical on every development machine. A constant
change moves goldens and therefore goes through `golden --update`, review,
and a cited clause exactly like every other build-affecting constant.

**Score what runs.** The primary input is the final relocated instruction
stream, after codegen, runtime insertion and layout — not a pre-layout asm
approximation and not an IR operation count. M11 adds a stable
`wrela dump --stage=cost` surface with totals by image, owner, function,
basic block and turn where those boundaries exist. Every term carries a
why-chain back through MachineWir/FlowWir to the source or synthetic
runtime site and names the profile rule that priced it. A diff such as
`proxy_cycles: 812 -> 852` must expand into the blocks, instructions and
constants responsible; a number without causality is not a review surface.

**IR passes use the backend as their scorer.** There is no second
"FlowWir cycle" formula to drift from emitted reality. The A/B harness
clones the same input, runs a pass off and on, lowers both candidates
through the ordinary backend and layout, and compares their final proxy
scores. A candidate-selection API does the same for bounded searches over
spill choices, instruction sequences or other local alternatives. Full
lowering is the dumb baseline even if it is slow; making candidate scoring
incremental, cached or IR-local requires a compiler profile and its own
regression lock like any other cleverness.

This captures every IR win that survives lowering into cheaper code:
removed copies and checks, folded constants, dead operations, reduced frame
traffic, better scalarization, and inlining or loop changes whose priced
path is known. It also correctly reports **no win** when later lowering
canonicalizes two different IRs to the same stream. Wins that depend on
unknown branch frequencies, data-dependent trip counts, cache locality or
cross-core interference are outside the first scorer's knowledge and are
labelled as such, never silently assigned false precision.

**Dumb model first; residuals buy detail.** Start with instruction costs,
dependency chains, issue width and explicit branch classes over
straight-line blocks and statically known control flow. Ports, ROB effects,
recording-weighted paths, cache simulation and prefetch are added only when
a pinned calibration pair proves the simpler model ranks alternatives
wrong. In particular, `ChoiceLog v1` does not contain branch outcomes or
memory addresses; any later replay-weighted cache model first needs an
explicit deterministic trace producer and its own dump/goldens. "Replay is
deterministic" is not permission to pretend that trace already exists.

**Calibration tests rankings, not numerology.** Each modeled effect has an
isolating A/B microbenchmark, amplified until the physical difference is
larger than measurement noise, with source, raw observations and derivation
committed beside the resulting constant. The calibration report's primary
oracles are pairwise ordering accuracy, false-win rate and correlation of
predicted deltas with measured deltas; it also carries a held-out set that
was not used to choose constants. A proxy win that a physical A/B turns
into a regression is handled like a fuzz find: minimize the pair, pin it,
then either fix the relevant rule or mark that class unscored. Never nudge
a constant until one workload goes quiet.

**Zero tolerance remains zero where the facts are semantic.** Recording
facts that already exist — choice entries, vCPU exits, transcript bytes and
exit status — compare exactly. If M11 wants checkpoint crossings or another
architectural count, it first records and pins that fact; `profile` does not
claim to measure something the recorder does not expose. These equalities
are separate from proxy calibration: semantic mismatch is a bug, while a
performance misranking is a model finding.

**06 §5's expected exit rates per device are in scope here as semantic
predictions.** They are architectural facts of a named recording, not
µarch estimates: the report predicts them, `profile`/`repro` compare them
exactly, and a mismatch is a bug. That retires the old ledger flip story
that waited on a "device milestone" for exit-rate lines — M11 owns the
prediction side and the exact comparison, same commit as the clause note
rewrite.

**Acceptance is asymmetric, but the proxy never merges code by itself.**
The deterministic score has no minimum-win threshold: one proxy-cycle in a
fully scored region is a real model delta, while any regression is a golden
diff that must be explained. Physical measurements do have noise. An
inconclusive before/after is not permission to promote the proxy; amplify
with more straight-line sites, improve the workload, or record that the
candidate is not yet physically measurable. The complexity gate is
unchanged: a small score win does not pay for a special case that fails the
regeneration test, and a large one does not excuse it either.

**The model proposes; the recording disposes.** Search may use the proxy to
rank a million candidates — that is what it is for. Landing still pays the
cleverness budget's full three-part price, including a physical
before/after on a named recording and a lock. The proxy is an additional
deterministic review surface, never the landing authority. Exploiting model
bias can win a search; it cannot merge.

The plan's ordered spine is: (0) the minimal normative correction that
distinguishes this optimization proxy from a future proof model, plus a
rewrite of the `compiler.costs.predicted-vs-measured` ledger note so its
flip condition matches this milestone rather than the stale "device
milestone exit-rate lines" story; (1) a stable final-stream inventory and
`--stage=cost` dump; (2) `wrela-cost-v1`, identity and the first
straight-line scorer; (3) why-chain goldens and deterministic/metamorphic
checks; (4) the IR pass A/B and candidate-scoring harness; (5) the
differential calibration corpus; (6) `profile` integration, report
expected-exit-rate lines, and ledger closure; then (7) only the model
features justified by pinned misrankings; and (8) one held-out
optimization capstone that proves the whole loop works. Useful invariants
include determinism, monotonicity when a profile cost rises, identical
streams scoring identically, and every score term naming an instruction
and profile rule.

**The capstone spends exactly one unit of cleverness: proven
constant-index bounds-check elimination.** A FlowWir pass removes a bounds
check only when the array length and index prove the access in range; it
adds no heuristic and an unproved case remains byte-for-byte on the naive
path. This is the deliberate first purchase because it exercises every
piece M11 exists to build without hiding behind a backend-only peephole:
pass off/on over the same IR, ordinary lowering and layout, a final-stream
why-chain showing the removed check and abort edge, a proxy delta, verifier
and `diff-eval` agreement, a physical before/after on an amplified named
recording, and a lock.

**Amplify without inventing loop machinery.** The obvious amplifier — a
tight loop over the site — collides with the machine: async back-edges
emit the checkpoint service and drown the delta, and synchronous loops
still need `sema.bounds.loops`, which this milestone deliberately does not
depend on. The capstone workload therefore amplifies by **unrolling or by
many independent straight-line sites**, so the removed checks dominate the
measured path without a new loop-bounds engine and without checkpoint
noise. Record that shape in the plan; do not rediscover it mid-item.

The capstone case is **held out of calibration** until the profile and
scorer are frozen, so it tests prediction rather than teaching the model
its expected answer. Three outcomes, all honest: (a) measurable physical
win and proxy agrees — land the pass under the three-part cleverness
price; (b) measurable physical delta and proxy disagrees — pin the
misranking, revise the model or its declared uncertainty, and rerun
(never waive, never tune against the held-out case); (c) no measurable
physical delta even after straight-line amplification — record that
finding and do **not** land the optimization, but the proxy is still
allowed to close if its calibration oracles and semantic exact-matches are
green. Direction agreement is required only when the physical delta is
measurable above noise. M11 may conclude that this candidate does not pay;
it may not conclude that a proxy which ranked a measurable case incorrectly
is ready.

Flips: `compiler.costs.predicted-vs-measured`, when `profile` shows proxy
and physical deltas for named A/B cases and exact comparisons for the
semantic counts it reports (including expected exit rates once the report
predicts them). Opens separate clauses for final-stream coverage, score
determinism, why-chains and differential calibration; one coarse clause is
not enough to certify the oracle. **Does not depend on
`sema.bounds.loops`.** Requires coordinated normative edits to 04 §6 and
06 §1 (and their README summary): the optimization cost model is a proxy,
not "the real microarchitecture" and not the mechanism that discharges
`@budget`. Minimal edits, same-commit clauses, REVIEW-QUEUE lines.

Non-goals: optimizing anything beyond the single capstone (M11 builds the
oracle, then spends once to validate it); proving `@budget`; a portable
WCET or host-conformance envelope; exact replay path/address reconstruction
unless calibration demands it; multicore contention; DVFS and thermal.

**Settled here: no learned policy inside the compiler.** A deterministic
oracle is exactly what learned optimization could exploit, but a weights
blob fails the regeneration test, hides the causality 04 §7 requires, and
systematizes Goodhart against the proxy. Run searches offline and ship
their reviewable result: a table, constant, or verified peephole. Bounded
exhaustive search may run under a budget and fail closed over it; register
allocation may combine a simple allocator, this scorer, and bounded spill
search. **ML may inform an artifact; it may never be an artifact** — the
same relationship the project already has with fuzzing.

### Recorded language intentions (not yet scheduled)

- **Inferred error sets** (stdlib milestone, via doc revision): extend
  "pub declares, private infers" — the doctrine receiver effects, pool
  names, generic contracts, and comptime legality already follow — to
  error types. A private `fn` omits its error type; the compiler infers
  the exact set from the closed world (it already computes this to erase
  impossible `CallError` variants, 02 §9.4); `pub` boundaries still
  demand a declared nominal enum. Lands when `Result`/`from` conversion
  machinery is real; it is a normative doc change first, human-reviewed.
- **Cost proofs** (deliberately not M11). M11's proxy ranks compiler
  alternatives; it does not discharge `@budget`, make
  `sema.bounds.loops` sound in cycles, or prove elapsed latency. Those need
  a separate static upper-bound model with explicit path, memory and
  interference assumptions, plus a normative decision about the existing
  `@budget(bound=...)` work/memory surface. An end-to-end
  `@latency_assert` needs that proof model and a defined host contract, not
  merely a well-calibrated optimizer proxy. Human-gated because promoting a
  useful estimate into a safety proof is exactly the sort of semantic
  change that must never happen by accretion.
- **Deferred until an ingredient exists**: graph-level flow policy waits
  until a concrete image needs a concrete named check — hardcode that
  check, no policy query language; whole-image snapshot/time-travel is a
  VMM feature to weigh after M6's recorder exists.
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
  — the stdlib, the runtime in wrela, the cycle proxy — is compiler and
  machine work, and a compositor would interrupt that rather than inform
  it. Pixels is the one item whose dependencies all point *backwards* with
  nothing pointing back: the compositor is guest wrela source, so it wants
  the stdlib's closed SIMD vector set ([05 §8.1](docs/language/05-library.md),
  whose NEON lowering 04 §6 already calls a backend obligation because
  "the flagship's compositor is its hottest loop"), and its inner loop is
  a named future hot spot (see the cleverness budget), so it wants the
  cycle proxy — with which "tune only after a frame exists to measure"
  stops being a deferral and becomes an ordinary budget purchase scored by
  the proxy and disposed by the frame recording.

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
  and it is the claim to defend. M11 makes tuning that path repeatable, but
  its proxy does **not** prove the tail at build time — that claim waits for
  the separate cost-proof work recorded above. Measure tails, not averages;
  a benchmark reporting only a mean is measuring the half of the story
  wrela does not win on.
- **The scheduler's own spend order** (recorded so it is not improvised
  the first time someone profiles a boot). Each step manufactures the
  evidence the next one needs: (1) M10 — the runtime becomes wrela, and
  the dispatch compare chain stops being hand-written by construction;
  (2) **measure** — `bench guest` over byte-identical transcripts gives the
  exact before/after that has never existed for this code, and M11 adds a
  zero-variance proxy-score diff **beside**, never instead of, the physical
  timing run; (3) the two
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
