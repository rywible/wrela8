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
history when it completes. Active plan:
[plans/M19.md](plans/M19.md) (optimization harness; proxy-only modes).
M18 is COMPLETE ([plans/M18.md](plans/M18.md); cycle proxy — differential
ISA ranking only). M17 is COMPLETE ([plans/M17.md](plans/M17.md); thin
entropy device + sync MWIR floor). M16 is COMPLETE
([plans/M16.md](plans/M16.md); stdlib maturity). M15 is COMPLETE
([plans/M15.md](plans/M15.md); variable cores + true concurrent vCPUs;
barrier clause remains gap per
[plans/BLOCKED.md](plans/BLOCKED.md)). M14 is COMPLETE (doc cut:
constructive progress theorem; no `plans/M14.md` — capability cut, not a
build-out; forward-ref golden still `image.graph.handle-dag` gap). M13 is
COMPLETE ([plans/M13.md](plans/M13.md)). M12 is COMPLETE
([plans/M12.md](plans/M12.md)). M11 is COMPLETE
([plans/M11.md](plans/M11.md)). Ladder design:
[docs/superpowers/specs/2026-07-27-post-m15-ladder-design.md](docs/superpowers/specs/2026-07-27-post-m15-ladder-design.md).

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
down from 4. **Revised by M15 (2026-07-27):** N becomes a sealed image
fact (`Image(..., cores=N?)`), not a machine-revision constant; M8's
number and baton remain the historical bring-up. The flagship host runs a
thin Linux underneath the VMM (core
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
(the ISA baseline is shared; `wrela-cost-v1` ranks that ISA stream
differentially — it is not an A76 absolute model) and rejected
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

### M11 — The image runtime, generated and generic
M10 closed with the console/abort path as wrela source and the floor at
26 words — but left **714 ImageStatic A64 words** across twelve per-image
Rust emitters in `codegen.rs` (`emit_rt_enqueue`, `emit_rt_run_one`,
`emit_rt_select_and_run`, `emit_rt_xsend`/`xreply`/`drain`,
`emit_boot_init`, deadline/checkpoint emitters, …) plus **94 words** of
entry-driver residue (`build_entry_driver`). The specialization medium is
still Rust: exempt from every stage dump, golden, and future optimization
pass. ROADMAP M10's "Rust-shaped hole" claim is therefore half-true, and
the REVIEW-QUEUE escalation that named the fork — accept ImageStatic
permanently, or dissolve it — is settled here as **dissolve**.

**The design, settled here.** After `@image` evaluation produces the
ImageGraph (already the report's input), the compiler renders those facts
a second time — not as the report, but as a **hidden wrela module of
facts only**: consts, `@layout(runtime)` types (array lengths are
literals or module-level consts per plans/M10.md decision 581), one
`@placed` static at a fixed `RTDATA_BASE`, and exhaustive `match`
dispatch ladders over comptime-known instance/method indices. The fixed
handwritten [`stdlib/core/runtime.wr`](stdlib/core/runtime.wr) imports
that module and holds every algorithm, generically indexing
`RT.turns[i]` / `RT.mailboxes[i]`. The generator is a pretty-printer — a
`String` — fed through the ordinary front end, so corpus, dumps, goldens,
and fuzz apply for free. Application authors never import or write the
config module; `wrela dump --stage=rtconfig` is the inspection surface.
Image authoring (`@image` / `img.actor` / `img.seal()`) does not change.

**The facts-only rule.** Generated code may contain consts, layout types,
one placed static, and match ladders over comptime constants — never
loops or decisions. Logic leaking into the generator is the Rust-emitter
disease in a new costume. Ledger clause; mechanically enforceable.

**Rejected alternatives, recorded once.**

- **Config-as-data / device-tree.** A binary table of counts and offsets
  that a fully generic runtime reads at boot. Loses because dispatch
  cannot be data without function pointers, which this language does not
  have and which would make the call graph opaque to every future pass.
  Once dispatch must be generated code, generating the facts beside it
  costs nothing extra.
- **Comptime metaprogramming** that lets `@image` emit the tables
  (Zig-style). Maximum new semantic surface for zero authoring benefit —
  every image author would live in a language where that capability
  exists.
- **Per-image generated algorithms.** The algorithms stay handwritten
  once in `runtime.wr`. If generated code starts containing loops or
  decisions rather than constants, types, and a match ladder, the
  facts-only rule has failed.

**Perf, stated honestly.** Expect the scheduler to get slower again
(bounds checks + spill-everything over a generic table vs baked
immediates). The `bench guest` lock on `boot-actors` (700000us threshold;
~62914us measured at M10 close) is the tripwire — do not pre-optimize to
dodge it. The payoff is structural: today's hand specialization becomes
*derivable* by ordinary passes — const-prop of the placed base and
constant indices, `TurnId` bounds-proof elision, dense-match jump tables
— each a future cleverness-budget purchase scored by **M18's** proxy,
acting on runtime and user code alike. M11 makes the runtime reachable by
the budget; it does not spend it.

**Machine-contract prerequisite.** A fixed `RTDATA_BASE` is
machine-revision-visible (`machine.layout.v1-constants`). Layout today
derives `rtdata` after code/rodata/abort/checkpoint; the config module
wants `@placed(...)` before codegen. Fixing the base (like `pages` at
`0x40000000` and `entry` at `0x40500000`) is the dumb resolution;
fixpoint layout is disqualified on sight. Flag the revision-bump question
in the plan rather than absorbing it.

**Migration discipline.** Layer 1 (`@budget` + console cleanup) is
independent and lands first — pure wins even if Layers 2–3 never ship.
Layer 2 is the generator + two-batch front end. Layer 3 migrates each
ImageStatic emitter into generic `runtime.wr` + generated facts,
easiest-first, with byte-identical boot transcripts as the differential
oracle before each Rust emitter is deleted (M10's own discipline). Late:
`@test(runtime)` as ordinary supervised root turns, deleting the
entry-driver residue. Detail: [plans/M11.md](plans/M11.md).

Opens: facts-only / generator-determinism / rtconfig-dump clauses (named
in the plan). Narrows: `sema.bounds.loops` (the `@budget` half for sync
loops). Non-goals: optimizing the scheduler (M18-proxy-gated); fusion;
`@naked`; comptime metaprogramming; config-as-data; register allocation.

### M12 — The representation rung (placed statics back to O(1))
Ladder rewritten 2026-07-26 (human-directed): the former M12 (authoring
hardening) is absorbed and dissolved by M12/M13 below — item 1 moves
into M13 as a dependency-ordered item; items 2–3 (epoch freshness,
restart intensity) lose their subject when M13 cuts per-actor restart.

M11's rule — *every runtime-varying reference is an index into a
statically-sized, uniformly-strided array* — holds for data and is faked
by numbered names wherever the element is a placed region:
`RING{0..7}_*`, `INIT_SLOT{0..31}`, `WAKE_PEND{0..7}`, each reached by a
generated accessor ladder. This short rung makes the rule hold for real:
uniform ring stride (bytes buy a type — the M10 turn-area trade, with
the offset-table fallback named and a fail-closed `RTDATA_SIZE_MAX`
gate), wake and init-span convergence, `GROUP_MAX_CHILDREN` from a Rust
const to an image fact (02 §9.5's own `capacity=4` example finally
runs), `@budget(bound=NAME)` so a loop bound and the const it shadows
are one fact, and a **placed-static census ratchet** as the exit
criterion. Every item is verified by byte-identical boot transcripts —
representation only. A generated *dispatch* ladder is forced (no
function pointers); a generated *data* ladder is a defect. Detail:
[plans/M12.md](plans/M12.md).

### M13 — One vocabulary, no silent failure (the unification milestone)
Apply the language's signature move — computed, published, checked — to
its own remaining exceptions, and let every item delete something.
Detail: [plans/M13.md](plans/M13.md). The load-bearing pieces:

- **The revision-boundary cuts.** Spec promises with no implementation,
  no clause, and no consumer are cut, each naming its replacement:
  `race(...)`, `@detached`, `@no_promote`, `group(budget=)`,
  function-level `@budget`, `@task(budget=|priority=)`, and the whole
  priority-band apparatus — 04 §2 is rewritten to state the shipped
  FIFO + round-robin semantics as *the* semantics. Machine-v1
  conformance narrows to blk/console/clock/entropy/input/display; net
  and sound become future machine revisions. Drivers land
  incrementally per device, each with its conformance boot golden,
  none blocking a rung.
- **Crash-only failure model** (human-decided 2026-07-26). Abandonment
  runs cleanup, is attributed, and the image applies its declared
  policy (`img.on_failure`: reboot or halt, required-explicit) —
  per-actor restart, 04 §5, provisions, intensity, and `PeerFailed`
  are deleted. Viability: we **accept bystander loss and pay with a
  durability requirement** (`Reboot` *presumes* app durable checkpoints
  via storage — named dependency, currently unbuilt; conformance
  goldens pin `Halt` until it exists). Not covered by the cut rule —
  restoring general restart later would be expensive, and the plan
  says so; the recorded fallback is **driver-only restart** (device
  reset epochs are its partial machinery; would re-add
  `Admission.Restarting`), evidence-gated on a recording where
  rebooting through a driver fault is unacceptable.
- **The vocabulary.** Struct field visibility enforced in three
  ordered items (census → migrate → flip; 05 §5's "opaque" `Instant`
  becomes true; sealed user types become mintable);
  `CallError`/`Admission` source-nameable; `Rejected` folds into
  `NotAdmitted`; no silent `Err` discard without `@discard(reason=)`;
  the proof-conditioned rule generalized (`reserve_proven` collapses
  into `reserve`; 05 §10's naming law deleted); the loop-discharge
  theorem replaces the event-loop name allowlist with a structural
  observes-bit; computed type classes replace the per-type containment
  prose; `resource(manual)` + private fields make user typestate real
  (`Validated` demotes to an honest idiom); method-owned generic
  parameters land (02 §8.3's flagship idiom compiles); inferred error
  sets for private fns (promoted from the intention below; its doc
  revision stays human-gated).
- **The ISR gate** — **decided 2026-07-28 (human): keep the
  user-visible ISR** (reject deletion). Evidence package was prepared
  (latency still `UNMEASURED`; golden inventory; 06 §5 tension; M14
  upward-edge sentence); accept space foreclosed without measured
  turn-boundary numbers on an IRQ-bearing boot. `InterruptCell`, ISR
  effect set, masked-interval report, and graph-changing `DriverMode`
  stay. Cycle-proxy (M18) unblocked on this gate. Named follow-on:
  surgical 06 §5 resolution (IRQ-partition status/ack carve-out
  preferred), not wholesale deletion. Detail:
  [plans/REVIEW-QUEUE.md](plans/REVIEW-QUEUE.md) ACCEPTED line;
  [plans/M13.md](plans/M13.md) appendix.

### M14 — Progress, constructively (no graph analyzer) — COMPLETE
**Done (2026-07-27).** Normative cut landed (04 §1 Progress rewrite;
02 §12.1 handle-cycle permission deleted; upward-edge sentence frozen;
ledger retargeted). Remaining honesty pin: `image.graph.handle-dag`
(forward-ref source golden). Settled 2026-07-26 (human decision,
superseding the same day's earlier draft of this rung as a build-time
wait-for-graph analysis): **the graph analyzer does not get built.**
That decision stands. What this rung is — stated in the same register
as M13's crash-only cut, because it is the same shape of decision —
is a **capability cut**, not a documentation of an accident.

**The cut (02 §12.1, named).** Normative text today says: "Construction
edges (moves, initialization order) must form a DAG; **handle edges
may be cyclic.**" Ledger clause `image.graph.construction-dag` pins the
construction half; the handle-cycle permission has been dead on arrival
since M4 — declaration-order evaluation makes cycles unconstructible,
and `decl.handle()` vs a bare decl-reference are the identical
`Value::ImageDecl`, so the shipped DFS refuses *all* cycles including
the ones the doc permits (recorded in that clause's note as an honest
finding). Zero consumers. M14 deletes the permission and states the
DAG as law.

**Counterargument preserved** (REVIEW-QUEUE line of the doc commit
carries it, not just the rationale): cyclic handles have a genuinely
safe use the DAG kills — **bidirectional peer `send`**. Sends are
fire-and-forget with no wait edge, so a send-only handle cycle cannot
deadlock. The cut is still justified (unconstructible, unimplemented,
zero consumers), but restoring that use later is *not* the five-node-
kind analyzer: it is a ~50-line static **await-edge** DAG check over
actor pairs (the construction-dag DFS reused, cycle printed). That
re-admission path is recorded so the cut is honest about its price.

**Why no analyzer still holds.** Per wait-edge kind after the cut: an
actor-call await can only cycle if handles cycle, and handles are a
DAG by the cut plus the `Actor[T]` mobility class (05 §2 — computed
after M13); group joins reduce to the same case; receipt awaits ride
device progress under the deadline/quarantine backstop (04 §1's
external-event carve-out); admission is fail-fast `NotAdmitted`, never
a wait; and **no blocking acquire exists** — `reserve` is synchronous
fail-fast-or-proven, and 03 §4 already states a driver "never awaits a
permit its own bottom half produces." A hand-spelled permit retry loop
dies by M13's loop-discharge theorem + turn non-reentrancy as a
budget-trip abandonment, not a deadlock. Starvation is FIFO +
round-robin plus the same loop theorem.

**The upward edge (unstated architectural rule, now stated).** Downward
edges are handles; **upward edges are resolutions or vectors.** A
client reaches a driver by handle (request); the driver answers by
resolving the client's parked call or receipt (reply); the device
reaches the driver by vector (06 §7's vsync-on-the-frame-vector). That
sentence is what makes the handle DAG survive contact with a real
appliance rather than being an artifact of every current golden being
request/response. It was an **input the ISR gate had to preserve or
replace**. **Gate decided 2026-07-28: preserve** — upward edge stays
literal (**resolution or vector**); the delete proposal (vector →
bottom-half wake) is rejected. Ordering note discharged: M14 froze the
answer space; the gate chose within it.

**What lands.** Rewrite 04 §1's Progress paragraph (and 04 §3/§4's
wait-for-graph phrasing) into the constructive theorem above, one
disposition per edge kind, including the up-is-a-resolution-or-vector
sentence (human-reviewed normative revision); delete 02 §12.1's
"handle edges may be cyclic" with the counterargument and re-admission
path in the REVIEW-QUEUE line; state + clause + golden-pin the two
load-bearing rules (handles are `@image`-wiring-only and never
rebound; the mobility class preserves the DAG — the M6/M8
forward-reference refusal graduates from a recorded experiment to a
golden); walk every suspension kind in FlowWir recording its
disposition, so a future blocking-acquire or rebinding construct meets
a fail-closed refusal naming this rung. Retargets
`compiler.progress.wait-for-graph` to the constructive clauses with
history preserved. Follows M13 because M13 lands its ingredients
(loop discharge, fail-fast admission, the `Actor[T]` class).

### M15 — Variable cores + true concurrent vCPUs — COMPLETE
Settled 2026-07-27 (human decision); closed 2026-07-27. Multicore today is affinity and
messaging under M8 decision 11's **baton** — exactly one vCPU inside
`hv_vcpu_run` at a time — with a machine-revision constant of **3 vCPUs,
always**. That buys sealed placement and cross-core rings without host
scheduler interleaving, and without parallel throughput; 04 §3's sealed
publish/acquire barriers are unobservable under the baton
(`machine.cross-core.publish-acquire-barrier` stays a gap). M11 deferred
`cores=N` authoring once `N_CORES` became a generated fact. This rung
owns both: **the image names how many cores it wants**, and **those cores
actually run together**.

**Doctrine revise (same-commit normative edit when the rung activates).**
Core count is **not** a machine revision and not ambient discovery. The
machine revision stays ISA baseline, memory-map *rules*, and the closed
device set (01's "new device or core count is a machine revision" narrows
to devices / map rules; 06 §1's "3 vCPUs, always" is deleted). N is a
**sealed image fact**: authored, placed into the report, digested into
build identity. The flagship housekeeping arithmetic
(`host cores − 1`) remains advice for choosing N on a Pi 5, not a
contract constant the image must equal.

**Authoring.** `Image(name=..., target=..., cores=N?)` — optional
`cores`, default **1** (today's single-core floor; existing goldens stay
quiet). `seal()` unchanged. Placement domain is `0 .. N-1`: explicit
`core=` pins first (`core ≥ N` → build error), then deterministic packing
(work → bytes → identity) as today. Virtio-blk and any other pin rules
keep their pins; out-of-range fails closed. Message APIs, handles, and
"no work stealing / no migration" do not change — parallelism is still
actors plus moved ownership, now with a real overlapping schedule in
stage 2.

**No compile-time max.** The contract does not name `VCPUS_MAX`. Layout
and the report grow with N. The VMM **refuses at boot** if the host
cannot provide N vCPUs (short host is a VMM error, never a guest probe —
06 §3). Pathological N is bounded the same way pathological mailbox
depths already are: fail closed when a concrete layout/reservation
ceiling is exceeded, not by pretending a machine-wide core max.

**Layout: report-owned stacks, fixed `IMAGE_BASE`.** Demote/delete
`wrela-machine::VCPUS` as a hard constant. Per-core stacks become report
lines (base/size), packed from **high DRAM** (or after the image) so
`IMAGE_BASE` / `RTDATA_BASE` stay put. Secondary `CoreEntry` lines,
release doorbell value, pending words, and ring topology all size to the
sealed N. Guest runtime keeps indexing M11's generated `N_CORES` — the
thin follow-up M11 decision 708–709 named. Cross-core images with
`cores=1` are a build error when any pin or edge demands another core.

**Two stages inside one milestone** (do not close after stage 1 alone).

1. **`cores=N` under the baton.** Normative + ledger first. Authoring,
   placement domain, report/layout parameterization, VMM creates N vCPU
   threads but still serializes `hv_vcpu_run` through the baton. Boot
   goldens for `cores=1` (byte-identical to today), `cores=2`, and
   `cores=3` (or higher on a capable host); host-too-small refusal pinned.
   Stage 1 is reviewable without rewriting the recorder — and must not
   claim "parallel throughput" while the baton remains.

2. **Delete the baton.** True overlapping `hv_vcpu_run` on ≥2 host
   threads. Emit inlined publish/acquire DMBs (`DMB ISHST` after ring
   payload/`count` stores and before the pending-word raise; `DMB ISHLD`
   after occupancy load and before payload reads in drain) via a
   runtime-only intrinsic — one word, no BL. Record Yield order as
   enumerable `ChoiceEntry::Progress` (park/release only) so replay
   serializes from the log; exhaustive schedule enumeration stays the
   M6 later intention (not this rung). Pin a boot golden whose
   transcript **fails under concurrent execution when those barriers are
   omitted**. Delete the `HV_VCPU_RUN_DEPTH` baton assert (overlap
   required on the concurrent golden).

**Ordered spine (coarse; plan freezes the walk).**
(0) Normative cut: 06 §1, 01 model sentence, 02/04/05 `Image`/`cores=`
surface, ledger opens. (1) Report `Cores` + high-DRAM stack lines.
(2) Compiler/layout/VMM parameterized on N; **delete `VCPUS`**.
(3) Authoring + placement goldens. (4) Stage-1 multicore boots under
baton. (5) Inline `@dmb` in publish/drain. (6) Baton deletion +
Yield-Progress replay. (7) Barrier-deletion golden; flip
`machine.cross-core.publish-acquire-barrier`.

Flips: authoring/report/host-refuse clauses (`image.cores.authoring`,
`image.report.cores-and-stacks`, `machine.vmm.host-cores-refuse`).
**Not flipped (host limit):** `machine.cross-core.publish-acquire-barrier`
remains `gap` — see [plans/BLOCKED.md](plans/BLOCKED.md) / known risk 4.
Touches:
`actors.placement.deterministic` (domain is N, not the constant 3).
Normative edits as above. Detail: [plans/M15.md](plans/M15.md).

**Non-goals.** Work stealing; actor migration; app-visible atomics or
fences; changing the placement *algorithm* beyond the N domain (work
proofs remain M-whatever lands them); cycle proxy / optimizer shelf
(M18/M19); exhaustive schedule enumeration / POR (M6 later intention —
M15 only needs Yield-`Progress` replay); KVM bring-up (still the
flagship-host intention); pixels; raising the flagship housekeeping
story into a second guest-visible contract.

**Rejected here (do not relitigate).** (a) Core count as machine revision
strings (`…_3cpu`) — explodes revisions for an image fact. (b) Compile-time
`VCPUS_MAX` in the contract — a soft host/layout ceiling may still fail
closed in code, but the docs do not publish a max. (c) Growing the low
stack slab and moving `IMAGE_BASE` with N — golden churn for no guest
benefit. (d) Closing the rung after `cores=N` while the baton remains —
that would repeat M8's "three cores, no throughput" claim under a new
name.

### M16 — Stdlib maturity — COMPLETE
Closed 2026-07-28. Make the stdlib and machine device story honest and
testable before spending the cleverness budget. Detail:
[plans/M16.md](plans/M16.md).

**Owns:**

1. **06 honesty rewrite** — split thin device contracts (clock, console,
   entropy) from queue/`@driver` contracts (blk now; input/display at
   pixels); say **virtio** only where the shipped contract is virtio.
   Stdlib README / ROADMAP echo in the same commit family.
2. **`stdlib/drivers/`** as sibling of `stdlib/core/`, imported via a
   reserved alias `drivers` mirroring `core` (e.g.
   `from drivers.blk import …`). Package tree contains **only**
   `@driver` modules.
3. **Relocate blk** out of inlined golden `@driver` bodies into
   `drivers.blk`. Move, don't redesign; existing boot transcripts remain
   the oracle (byte-identical or deliberately re-pinned with a ledger
   cite).
4. **Dual-tier in-wrela suite:**
   - *Comptime:* `@test` / `@test(exhaustive)` under `stdlib/tests/`
     over pure `core` (List, time constructors, Format helpers, …). No
     VMM.
   - *Runtime:* owned blk boot goldens import `drivers.blk` and pin
     transcripts.
5. Wire the suite into `cargo xtask check` as lane **`stdlib-test`**.
   Empty suite root fails closed.
6. **Console / clock:** name them as thin contracts; floor cleanup only
   if required for honesty. No console `@driver`. No clock `@driver`.

**Vocabulary (settled).** *Device* = VMM model + report/conformance.
*`@driver`* = guest actor root owning a multiphasic device protocol.
*Thin guest surface* = sealed API for devices that do not earn a
`@driver` (`now()`, console ring helpers, entropy effect). All
machine-v1 rows are devices; only some necessitate a full `@driver`.

**Explicit non-goals:** entropy (M17); input/display; cleverness-budget
spends; KVM; replacing virtio-blk; inventing a general device framework;
console/clock/entropy as `@driver`s.

**Exit criteria (coarse; plan freezes the walk):** no inlined virtio-blk
`@driver` bodies left in the blk boot goldens M16 owns; comptime suite
green under `check`; at least one runtime golden imports `drivers.blk`
and matches its pin; 06/README/ROADMAP no longer claim every device has
a stdlib `@driver` or that console/entropy are virtio; ledger clauses
for packaging + suite opened/flipped as the plan names.

### M17 — Entropy — COMPLETE
**Done (2026-07-28).** Thin entropy **device** — not a `@driver`, not
virtio-rng rings. Detail: [plans/M17.md](plans/M17.md).

**Owns:**

1. Thin entropy device in the VMM: recorded-source model (live host
   entropy; replay from the choice log; fail closed on underrun — clock
   underrun is the precedent).
2. Sealed guest runtime effect (name/API frozen in the activation plan;
   roughly `entropy(n) -> Bytes`), illegal at comptime/ISR like
   `now()`, lowered to a small fixed machine contract (trapping MMIO or
   equally small path — **no** virtqueues required for v1).
3. Boot + replay golden that diverges if bytes are not logged/replayed.
4. Normative row under thin device contracts (if not already placed by
   M16's 06 split with entropy marked "lands M17").

Does not add modules under `stdlib/drivers/`. Why its own rung: new VMM
model + guest intrinsic + recorder path + conformance golden is still a
real surface; M16's packaging/test exit must not wait on it. Folding
into M16 remains a human call at activation if entropy is truly tiny —
default is split. Plan when activated. Non-goals: input/display;
net/sound; virtio-rng rings; `stdlib/drivers/` changes.

### M18 — The cycle proxy — COMPLETE
**Closed 2026-07-28.** Detail: [plans/M18.md](plans/M18.md).

**Prerequisite discharged (2026-07-28):** the M13 ISR gate chose **keep**
— ISR / `InterruptCell` paths are not scheduled to vanish. **Shape
frozen 2026-07-28 (human):** differential **ISA** proxy only — **no
physical calibration**, no `profile` / `bench guest` work, **not an A76
(or Apple) microarchitecture model**.

This milestone shipped a deterministic **proxy-cycle** score for
**ranking** emitted code. One question only:

> Given two semantically equivalent emitted programs, which ranks lower
> under `wrela-cost-v1`?

**The value is differential, not absolute.** Proxy-cycles are not claimed
wall-clock on Pi 5, Mac, or anything else. Cache sizes, predictors, and
µarch details change real time; they must not be required to preserve
**rank direction** for the emissions we care about (fewer / cheaper ops,
shorter true data deps → lower proxy on every conforming host). Absolute
accuracy vs silicon is out of scope. Physical / host wall-time is
**not** part of the optimization process (see cleverness budget / M19);
optional offline research may retune `wrela-cost-v1` on proxy misrank
suspicion only.

**Target the sealed ISA stream, not a chip.** The machine’s guest ISA is
the ARMv8.2-A + NEON baseline (06 §1 — intersection of A76 and Apple
Silicon). `wrela-cost-v1` ranks that **emission**, the same bytes whether
the VMM is HVF or a Pi. Do **not** paste A76 Software Optimization Guide
port maps into the proxy; do **not** calibrate against HVF timings. 06 §1
and 04 §5 **split** the flagship A76 product/backend / `@budget` story
from this **ISA ranking proxy** (landed M18 item A).

**`wrela-cost-v1`.** A versioned parameter file beside the machine
constants (`bench/thresholds.toml`'s precedent): per-`CostRule` **latency**
(and optional coarse throughput / a single model `issue_width`) for the
baseline ISA op classes. Unit: the **v1 proxy-cycle** — defined by the
file’s scheduler, not by a host. Digest seals the **cost dump / report
side** (and goldens); it does not reseal the unsigned image unless a
later rule says so. Refine rows only when a pinned **proxy** misrank
demands it — never by fitting wall time. This file is the **ranking
proxy**, not `@budget` discharge and not (yet) the report’s copy-price
surface (04 §1).

**Score the final stream.** After codegen, runtime insertion, and layout
— not pre-layout asm, not an IR op count. Every emitted word carries
emit-time **`CostRule` + dest/src regs** (always on; no cost-profile
toggle; never parse mnemonics). Scorer: a dumb **register scoreboard**
over the stream (`start = max(ready[srcs], issue_constraint)`;
`ready[dst] = start + latency(rule)`; fn total = time the last insn
retires). Optional fixed `issue_width` in the file is a **model
parameter**, not “A76’s real decode.” Assumptions stated in the dump
header (e.g. ignore cache hierarchy; no mispredict model — ChoiceLog has
no addresses/path weights). Stable `wrela dump --stage=cost` with Terms
(rule counts) + schedule totals by function and owners (**app /
generated-runtime / driver**). The image report prints a **short
summary** by default (version, digest, totals by owner) — not per-Term
lines. Floor: "which rule, which count, what schedule total."

**A/B through ordinary lowering.** Clone the input, flip one change, lower
both the usual way, compare final proxy scores. No second "FlowWir cycle"
formula. No candidate-search API — off/on validates the ruler and the
capstone; bounded search is a later budget spend on the same score
function.

**No physical calibration.** Do not amplify cases until wall-time deltas
beat noise; do not tune `wrela-cost-v1` against `bench guest` / `profile`
on any host. Proxy regressions are golden diffs. Proxy wins have no
minimum size and are **not** physical evidence. Host wall-time is
out of the opt loop (M19 / cleverness budget). If offline research ever
suggests the proxy misranks, retune `wrela-cost-v1` deliberately — never
calibrate M18 against HVF. Leave `profile` untouched for the ruler.

**Semantic counts stay exact.** Choice entries, exits, transcript bytes,
exit status: exact match or bug (correctness oracles beside the proxy).
Checkpoint crossings only after the recorder exposes them. Exit-rate
*predictions* in the report (06 §5) are **not** M18. Rewrite the stale
ledger note so this clause flips on `--stage=cost` / report summary +
proxy off/on A/B + semantic exact-matches — **not** on exit-rate lines
and **not** on predicted-vs-physical pairing.

**Ordered spine.** (0) Normative fix landed: ISA ranking proxy ≠ A76
absolute model ≠ `@budget` discharge; ledger opens + flip note. (1) Emit
tags+regs + `--stage=cost`. (2) `wrela-cost-v1` + scoreboard scorer. (3)
Determinism / golden dumps + report summary. (4) Pass off/on proxy A/B.
(5) Small differential corpus (rank order only). (6) Capstone below.
Detail only for pinned *proxy* misranks.

**Done when** the dump, scoreboard, differential A/B oracles, report
summary, and ledger flip are green — independent of whether any opt lands
under the budget.

**Capstone: proven constant-index bounds-check elimination.** One
FlowWir rewrite; unproved sites untouched. Score with many straight-line
sites — not async loops (checkpoints drown the delta) and not sync loops
(`sema.bounds.loops` out of scope; M11 narrows that gap for authoring).
Held out of the differential corpus used to sanity-check the scorer.
**Proxy smoke:** off/on must improve **rank** (lower schedule total) or
the model is wrong. **Landing** still pays the cleverness budget and may
wait for M19's mode harness; host timing does not block M18 close.
Never tune `wrela-cost-v1` against the held-out case.

Flips: `compiler.costs.predicted-vs-measured`. Opens only what the dump
and scorer need. **Does not depend on `sema.bounds.loops`.** Normative
edits to 04 and 06 §1 (ISA ranking proxy vs flagship A76 wording) —
**done** with M18 item A.
Non-goals: physical / host calibration; A76 SOG port maps as the proxy;
cache/L2/L3/branch-mispredict models; `profile` juxtaposition; `@budget`
proofs; WCET; exit-rate report lines; multicore contention; DVFS/thermal;
in-compiler ML; anything beyond the capstone smoke.

### M19 — The optimization harness — ACTIVE
**Activated 2026-07-28.** Detail: [plans/M19.md](plans/M19.md). M18 is
the ruler (COMPLETE); M19 is the **in-code harness** that uses it — not
an evidence table, not purchases, not a physical-measurement loop.

**What lands.**

1. **Two compile modes, hardcoded.** `dev` = every named opt off;
   `release` = every named opt on, in a fixed call order that lives in
   the compiler (ordinary named functions — no pass manager, no recipe
   TOML, no plugin system). Default product path is `release`.
2. **Proxy is the only profitability oracle.** Against the fixed cost
   corpus: the release pipeline vs `dev` (and a candidate pipeline vs the
   current release set) must **not raise any case's proxy total** and must
   **strictly lower at least one**. Losers are deleted or reworked — not
   kept disabled "for later." Convolution matters: score the full
   pipeline in context, not isolated toy wins. Pass reordering stays in
   scope as an offline edit of the in-code order + re-rank; M19 does
   **not** ship a searcher.
3. **Both modes stay correct.** `dev` and `release` both pass existing
   oracles (`diff-eval`, goldens, validate / layout verifies under
   `cargo xtask check`). Semantics must not depend on opts. Production
   `wrela build` keeps only the cheap structural checks it already runs
   — it does not re-verify IR after every clever step. Ill-formed IR from
   an opt is a CI failure; well-formed wrong code is a `diff-eval`/golden
   failure.

**Physical / host wall-time is out of this process.** Optional offline
research may inform retuning `wrela-cost-v1` when a *proxy* misrank is
suspected — that improves the ruler; it is never a land gate, never a
`check` column, never wired into the harness.

Done when a later spend can add a named function to the release order,
prove the corpus proxy win rule, and leave `dev`/`release` green —
without inventing process. M19 ships **two** smoke opts so convolution
is real: M18's bounds-elide plus **narrow immediate materialization**
(`FnCtx::load_imm` skips zero `MOVK` halfwords; naive four-word form
stays `dev`). Close records the cost-corpus proxy Δ (`dev` → `release`).
Do not invent a third opt to prove the harness.

**Non-goals.** `optimizations.toml` / evidence rows; physical A/B or
`profile`/`bench guest` as opt gates; offline pass-order searcher;
register allocation; isel catalogs; fusion; weighted search; recipe
frameworks; SelectionDAG; cost proofs; exit-rate report lines;
"host all future lanes"; world's-fastest exit criteria. Those (where
still wanted) are later spends that use this harness, or recorded
intentions — not M19 deliverables.

Depends on M18's score + proxy A/B (COMPLETE; no physical calibration).
Opens: compile-mode and proxy-win-corpus clauses (ids in the plan).

### Recorded language intentions (not yet scheduled)

- **Inferred error sets** — *scheduled 2026-07-26: M13 (plans/M13.md
  item K)*; normative doc revision landed (02 §5 / §6.2 / §7.4, 04 §7,
  05 §1) and remains human-reviewed / rejectable without downstream
  effect. The intention as recorded: extend "pub declares, private
  infers" — the doctrine receiver effects, pool names, generic
  contracts, and comptime legality already follow — to error types; a
  private `fn` writes `-> Result[T]` and the compiler infers the exact
  set from the closed world (it already computes this to erase
  impossible `CallError` variants, 02 §9.4); `pub` boundaries still
  demand a declared nominal enum.
- **Cost proofs** (deliberately not M18). M18's proxy ranks compiler
  alternatives under an **ISA-level** `wrela-cost-v1` scoreboard
  (differential direction only; not A76 absolute; no host calibration);
  it does not discharge `@budget`, make
  `sema.bounds.loops` sound in cycles, or prove elapsed latency. Those need
  a separate static upper-bound model with explicit path, memory and
  interference assumptions, plus a normative decision about the existing
  `@budget(bound=...)` work/memory surface. (M11 narrows `sema.bounds.loops`
  for the authoring/runtime half — proven finite sync loops via `@budget`
  — and M13's loop-discharge theorem discharges the event-loop half
  structurally, without claiming cycle proofs.) An end-to-end
  `@latency_assert` needs that proof model and a defined host contract,
  not merely a well-calibrated optimizer proxy. Human-gated because
  promoting a useful estimate into a safety proof is exactly the sort of
  semantic change that must never happen by accretion. The same gate
  covers **hard copy budgets** — after M13 cuts function-level
  `@budget`, copy pricing is report-only until this proof model exists.
- **Default arithmetic as `Result`** (deliberately not M13). M13 forbids
  silent *error discard*; flipping abandoning `+`/`/`/shifts to
  `Result`-returning defaults is a separate normative + corpus-wide edit,
  human-gated like cost proofs — and M13's crash-only decision leans the
  other way on purpose (an overflow is a recorded, replayable, fatal bug,
  not a value).
- **Report expected exit rates** (06 §5; deliberately not M18). The report
  "states expected exit rates per device" is a semantic prediction with an
  exact `profile`/`repro` comparison — valuable and dumb, but it is report
  work, not the cycle proxy. Schedule when a named device recording needs
  the line; do not smuggle it into the scorer milestone.
- **06 §5 ISR hot-path MMIO carve-out** (recorded 2026-07-28 with the ISR
  gate keep decision). 06 §5 says MMIO exists only on setup/reset; 03 §6's
  ISR example does hot-path `interrupt_status` / `interrupt_ack`. Preferred
  fix: name IRQ-partition status/ack as an allowed exception (03 §2 already
  scopes that partition to what the ISR needs). Alternate: pending-word ack
  while keeping a user ISR for signal/`wake`. Not an ISR-deletion reopen;
  not M18.
- **Consumer-gated library surface** (recorded 2026-07-26; each waits
  for its first real consumer, and each has or gains an honest ledger
  clause via M13 item E so none is merely believed): `Completion[T]` as
  a user-facing type (M13 names the underlying resolution cell; the
  type ships with its first consumer); `Secret`/`Envelope` enforcement
  (`values.marked.secret` — first image holding a real secret);
  `SlotMap` minted-id semantics (`library.collections.slotmap-minted-id`
  — first contended multi-client service); messages taking non-`own`
  resources (`actors.messages.take-non-own-resource` — first driver
  that must hand a non-pooled resource across an edge); scoped pools /
  `with pool` (the remaining flip condition of
  `values.regions.two-binding-disciplines` — travels with the pixels
  rung, whose compositor is the natural first consumer).
- **Event-gated hardening** (recorded 2026-07-26): quarantine execution
  and in-flight receipt delivery on cancellation
  (`hardware.cancellation.recovery-turn`, mechanism simplified by M13's
  cuts to "deliver to the owning driver's existing bottom-half `@task`"
  — flips on the first cancellation-under-load golden; quarantine also
  adds `Admission.Quarantined`); and **driver-only restart**, the
  crash-only decision's recorded fallback — device reset epochs are its
  partial machinery; would re-add `Admission.Restarting` (no
  `Cancelled` overload); flip witness is a recording where rebooting
  through a mid-session driver fault is demonstrated unacceptable.
  Separately, the **durable-checkpoint idiom + storage stack** is the
  named dependency of crash-only's `Failure.Reboot` viability
  (currently unbuilt; conformance goldens pin `Halt` until it exists).
- **True concurrent vCPUs + variable image core count** — *COMPLETE
  2026-07-27: M15* ([plans/M15.md](plans/M15.md)). Delivered `cores=N`
  on `Image`, report-owned high-DRAM stacks, baton deletion, inline
  `@dmb`, Yield-`Progress` replay. **`machine.cross-core.publish-acquire-barrier`
  remains `gap`** (mutation not observed on HVF — plans/BLOCKED.md).
  Do not relitigate inside M16–M19.
- **Report/diagnostics coverage** (owner: report work, no rung): the
  actor-chatter lint (04 §7's `warning[performance]`) and the copy
  pricing threshold line (04 §1/§7) — M13 item E audits both and opens
  `compiler.diagnostics.actor-chatter` / `compiler.report.copy-pricing`
  as `test` or honest `gap` accordingly.
- **Post-cut re-additions** (recorded 2026-07-26 so M13's cut commits
  point somewhere): priority bands, `must_service_within`,
  priority/deadline inheritance, and `@task`/`group` budgets return
  only as cleverness-budget purchases against a recording that shows a
  missed deadline — the scheduler is wrela source since M11, so the
  budget applies to it like all code; `race(...)` returns as "selection
  with a loser-cancellation witness" when a consumer exists (the M13
  resolution cell makes it cheap).
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
  "aarch64"))]`. Development boots on this ladder use Hypervisor.framework
  on a Mac until a human schedules otherwise, and
  `xtask check`'s boot/repro/diff-eval/bench-guest lanes fail honestly
  (never silently skip) on any other host. So the ladder's
  development host is not the product's host. Recorded as a known,
  deliberate gap so it is a decision rather than an oversight; scheduling
  it is a human call. Shape decided 2026-07-24 (see M8): the flagship runs
  a **thin Linux under the VMM** — core isolation, VFIO passthrough for
  devices, one core pinned to housekeeping — never bare metal. wrela owns
  three cores and every device contract; Linux is a bootloader, an IOMMU
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
  the work is hard: it is that nothing else needs it.   Every remaining rung
  — stdlib maturity, entropy, the cycle proxy, the optimization
  playground — is compiler and machine work, and a compositor would
  interrupt that rather than inform it. Pixels is the one item whose
  dependencies all point *backwards* with nothing pointing back: the
  compositor is guest wrela source, so it wants the stdlib's closed SIMD
  vector set ([05 §8.1](docs/language/05-library.md), whose NEON lowering
  04 §6 already calls a backend obligation because "the flagship's
  compositor is its hottest loop"), and its inner loop is a named future
  hot spot (see the cleverness budget), so it wants the cycle proxy and
  the playground — with which "tune only after a frame exists to measure"
  stops being a deferral and becomes an ordinary budget purchase scored by
  the proxy, registered as evidence, and disposed by the frame recording.

  *What descheduling leaves open, stated rather than implied.*
  `machine.display.golden-frames` is a gap **no rung owns**, recorded as
  such in the clause's own note (`compiler.progress.wait-for-graph` was
  the same shape until 2026-07-26 gave it rung M14 — now COMPLETE as a
  constructive theorem; this one still waits here). 06 §10 lists the golden-image display tests in the machine
  conformance suite, so machine v1 is not conformant until this lands —
  a set M13 item D narrows: `net` and `sound` move to future machine
  revisions, leaving display and input (this bullet's own subjects) —
  and **entropy**, which **M17** owns as a thin recorded-source device
  (not virtio-rng; not a `@driver`) — as the remaining v1 contracts.
  The stdlib's closed SIMD
  vector set gets its honest clause the same way (`library.simd.
  vector-set`, opened by M13 item E, owner: this bullet). And the VMM's
  cross-device pool oracle stays half a unit test:
  `devices::tests::a_window_bound_to_another_device_is_refused_by_name`
  becomes a *boot* only once a second device model exists, whichever
  device that turns out to be.

  *The scope, if it is ever scheduled: **headless**.* Software scanout
  into memory and golden frame digests — never open a window, no GUI
  dependencies. That constraint was GOAL.md's standing rule while this was
  a rung and is preserved here so it is not rediscovered.

## The cleverness budget (permanent)

Cleverness is a resource, acquired only through the **cycle proxy**
(M18). An optimization lands only when all of the following hold, no
matter how obviously fast it looks on a host:

1. it is a **named skippable call** in the fixed in-code release pipeline
   (M19 harness — ordinary functions in a fixed order; `dev` turns them
   all off, `release` turns them all on);
2. enabling it **in context** (full release pipeline vs without it, or
   vs `dev`) does **not raise** proxy-cycles on any case in the fixed
   cost corpus and **strictly lowers** at least one — losers are scraped
   or reworked, not kept around disabled; and
3. correctness oracles stay green with opts on (`release`) and with all
   opts off (`dev`) — `diff-eval`, goldens, validate. Semantics must not
   depend on opts.

**Physical / host wall-time is not part of this process.** Flame graphs,
`profile` guest timings, and `bench guest` A/B are optional offline
research that may inform a deliberate retune of `wrela-cost-v1` when a
*proxy* misrank is suspected. They are never a land gate, never an
evidence-table column, and never something `cargo xtask check` demands
for an opt. Do not build physical measurement into the optimization
loop.

The **compiler lane** — how fast wrela code *compiles* — is separate and
still locked by `wrela --timings` / `xtask bench compiler` (M1). No
interning, arenas, parallelism, or incrementality in the compiler until
*that* bench shows the hot spot. Working hypothesis, falsifiable by the
compiler lock: the dumb compiler is already fast, because the things that
make compilers slow — LLVM, incremental machinery, heavy optimization
passes — are exactly the things this one does not have. Guest wall-time
lanes (`bench guest`, `profile`) remain available as **product / VMM**
measurement (boot thresholds, transcript identity); they do not gate
compiler opts.

**Spend shape.** *Floor* (always-on local truths under `release`),
*search* (bounded choice under the proxy — offline; not an in-compiler
enumerator at M19), *specialization* (recording-weighted ideas stay off
until paid under the same proxy win rule). Do not mix them. The naive
pipeline (`dev`) stays the reference; opts are named skippable calls;
miscompiles are `check` oracles (`diff-eval`, goldens, validate), not a
mandatory production verify sandwich; single-ISA recipes are fine; a
general mid-end is not; ML may emit tables offline and must not run in
the compiler.

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
  scheduler dispatch) wait their turn: the **proxy corpus** says when a
  candidate wins in context, and until then dumb code calling stdlib SIMD
  ops is the answer. After M19 they land as named calls in the release
  order, not as silent pipeline edits and not as table rows with physical
  columns.
- **Where I/O effort is worth spending, and where it is not.** For
  storage, the software path is already below the device's noise floor — a
  ~1 µs round trip against a 10–80 µs NVMe read is 1–5%, so optimizing it
  further buys something invisible and spends budget that has somewhere
  better to go. For networking, where wire times are sub-microsecond,
  software dominates and the zero-exit/zero-copy/vDPA path is where the
  wins actually are — **note (2026-07-26): `net` is outside machine-v1
  conformance after M13 item D** (future machine revision); this bullet
  and the M8 bandwidth arithmetic (~2M descriptors/sec/core, VFIO/vDPA)
  reason about that future revision, not the v1 contract M18/M19 first
  rank under the proxy. Prefer proxy-visible emission wins on the paths
  that dominate; do not invent host-timing theatre for noise-floor work.
- **The win is the tail, not the mean** — and this reframes what "beating a
  general-purpose OS" means. Throughput parity with a tuned Linux is
  achievable and unremarkable. What a general-purpose OS cannot offer is a
  flat p99.9: its tail is dominated by scheduling, interference, page
  faults, and allocator behavior, and wrela has none of those by
  construction. That win is not earned by optimization; it is already true,
  and it is the claim to defend. M18 makes ranking that path's *emissions*
  repeatable (zero-variance proxy A/B), but its proxy does **not** prove
  the tail at build time — that claim waits for the separate cost-proof
  work recorded above. Do not smuggle physical timing back in as a
  substitute for that proof.
- **The scheduler's own spend order** (recorded so it is not improvised
  the first time someone wants a faster boot). Each step manufactures the
  evidence the next one needs: (1) M10 — console/abort as wrela, uniform
  turns, floor locked; ImageStatic specialization remains as the
  recorded interim; (2) M11 — generated config + generic `runtime.wr`;
  ImageStatic words ratchet to zero and the dispatch compare chain becomes
  ordinary wrela `match` by construction; (3) M12–M13 — the
  representation rung (data ladders die, the census ratchets) and the
  vocabulary milestone (discarded `CallError` is refused in runtime and
  app code alike); (4) **rank** — M18's zero-variance proxy-score diff
  over the cost corpus (and, after M19, `dev` vs `release`) decides
  candidate order; `bench guest` may still lock product boot thresholds
  but does **not** gate opts; (5) the two dumb wins, if and only if the
  proxy win rule admits them — populate the already-reserved ready-queue
  table (O(actors) scan → O(1) pop, no layout change, the slots are
  placed already) and lower a dense comptime-known `match` to a jump
  table (one codegen change that lifts every `match` in the language, not
  a bespoke scheduler hack); (6) only then consider fusion, as a
  FlowWir → mwir *lowering* validated by `diff-eval`, never a rewrite.
  Nothing in this list needs `WFE`, an interrupt controller, or a global
  state machine — see M10's settled rejections.

Also permanently out: abstractions serving futures that are not ledger
clauses; incremental/parallel/cached anything in the compiler until a
profile of the *compiler* demands it; second ways to do things that have
one way; "temporary" relaxations of fail-closed.
