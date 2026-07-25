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
history when it completes. The active plan: [plans/M7.md](plans/M7.md).

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
Placement inference, cross-core rings, 4 vCPUs, per-mailbox admission
recording. Flips: `actors.placement.deterministic`.

### M9 — Pixels
Display + input devices, a dumb scalar tile compositor, golden frame
digests. SIMD/NEON tuning only after a frame exists to measure. Flips:
`machine.display.golden-frames`.

### M10 — The stdlib
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

### M11 — The runtime in wrela
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

Everything above the floor needs one narrow intrinsic surface — raw
load/store at a comptime-known address, plus the handful of system
instructions above — normative doc change and ledger clause first, in its
own commit, before any migration. **Rejected: `@naked` + inline asm.** It
is a real language feature with real semantic weight (no prologue, no
stack, register discipline sema cannot verify) — a large surface to add to
a language whose premise is that it checks everything, bought only to
delete twenty reviewable, byte-pinned words.

**Migration discipline (what makes this safe at all).** One routine at a
time, console formatter first (lowest risk, immediately testable), the
scheduler last. Each routine's wrela version must produce a
**byte-identical boot transcript** against every existing boot/replay
golden *before* its hand-assembled version is deleted — the transcripts
already pinned by M5–M9 are the differential oracle, and the hand-asm
implementation is the reference the new one is diffed against, exactly as
`diff-eval` uses the evaluator against the backend.

Opens: `runtime.*` clauses (there are none today — every one is opened
here). Non-goals: self-hosting the compiler; touching codegen; and
optimizing the scheduler — M11 makes the scheduler *reachable* by the
cleverness budget, it does not spend it. The first optimization pays the
full three-part price like everything else.

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
  call.

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
  and — once M11 lands — the scheduler, which runs between every turn and
  is unreachable by this budget until it stops being hand-assembly) wait
  their turn like everything else: the profile says when, and until then
  dumb code calling stdlib SIMD ops is the answer.

Also permanently out: abstractions serving futures that are not ledger
clauses; incremental/parallel/cached anything in the compiler until a
profile of the *compiler* demands it; second ways to do things that have
one way; "temporary" relaxations of fail-closed.
