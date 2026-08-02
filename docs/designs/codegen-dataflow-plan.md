# Codegen dataflow and late optimization plan

Status: implemented with gated/parked transforms
Audience: an implementation agent that should not need to invent algorithms or
infer ordering  
Scope: stable CFG/liveness dumps, FlowWir suspension liveness, state-local
register allocation, frame-slot coloring, scalar replacement, late
relocation/address relaxation, and a later re-evaluation of block layout and
bounds-check elimination

## 1. Goal

Build the shared dataflow and late-layout foundations needed to make codegen
smaller and faster without weakening Wrela's fail-closed behavior.

The work is deliberately split into small commits. Every analysis gets a stable
text dump and golden coverage before code generation consumes it. Do not combine
the read-only analysis commit with the transforming commit.

The intended dependency order is:

```text
cost-ruler precision preflight
        |
        v
shared instruction facts
        |
        v
stable CFG + ordinary liveness
        |
        +----------------------+
        |                      |
        v                      v
FlowWir suspend liveness   range proofs
        |                      |
        v                      v
state-local regalloc       bounds elimination
        |
        v
frame-slot coloring
        |
        v
scalar replacement

late linker representation -> relocation/address relaxation

CFG-based regalloc + exact emitted block bridge
        |
        v
re-ask hot/cold block layout
```

Scalar replacement is placed after frame coloring so coloring can first land as
an independently measurable optimization. Once scalar replacement lands, run
coloring again on the rewritten program; scalar replacement should expose more
short-lived scalar homes.

## 2. Cost-proxy audit and precision preflight

### Audit verdict

The current proxy has a strong deterministic skeleton:

- instruction-group rows are provenance checked;
- scheduling includes ports, dispatch width, bounded reordering, register and
  NZCV dependencies, branch terms, memory classes, and cross-core terms;
- emitted words carry explicit cost metadata;
- the flat and measured block-composition formulas are tested;
- the residual parameter box and correctness-ordering vetoes fail closed.

It is not yet precise enough to be the sole ruler for the transformations in
this plan. The following are correctness-of-measurement issues, not requests for
more microarchitectural detail:

1. A `MOVK` preserves the destination's other bits and therefore reads the old
   destination value. Current emit sites give it no source register, so a
   `MOVZ; MOVK; MOVK; MOVK` materialization is scored as four independent
   operations. Immediate relaxation is consequently measured against the wrong
   dependency graph.
2. `score_words` creates a fresh `MemState` for each emitted basic block. The
   first frame access in every block is therefore charged as a new compulsory
   access even when adjacent executed blocks use the same hot frame line. This
   can over-credit register allocation, frame coloring, scalar replacement, and
   bounds elimination.
3. The shipped-program cost path scores `ImageCodegen.program` before
   `layout_program` injects enqueue/dispatch, cross-core, boot-init, and
   checkpoint/IRQ functions. Entry, abort-landing, and checkpoint section words
   are raw words outside `CodegenProgram` and are not scored at all.
4. The I-side footprint repacks and rounds every function to a 64-byte boundary,
   but the real linker concatenates functions without that padding. The budget
   therefore uses neither the real linked addresses nor the real cache-set
   mapping. It also makes pure function repartitioning appear expensive for
   padding the image does not contain.
5. `wrela dump --stage=cost` on an image root currently reports the cost-stage
   closure, while the image report reports a different live image program.
   Optimization attribution mixes these two scopes in different helpers.
6. The named `boot-actors` workload is composable in the cost library but is not
   wired into the actual whole-corpus optimization comparison. The committed
   block vector currently resolves only `893/6647` hit mass, and its uncovered
   maximum-function penalty contributes more than 99% of that row. Such a row
   is honest about missing coverage but is not a precise profitability ruler.
7. `turns_per_sec` and `ms_per_turn_model` are derived from the maximum single
   entry-method schedule. They omit dynamically called functions, scheduler
   work, and measured block frequency, so they are not whole-turn throughput
   estimates.
8. A call currently clears the modeled L1D/store-buffer window even though a
   call instruction does not flush L1D. Stack-pointer writes do the same. This
   can over-credit inlining, tail calls, frameless emission, and any transform
   that removes a call or frame adjustment.
9. Store-buffer depth is derived as `cap_l_each * number_of_load/store_pipes`.
   An issue-capacity row is not a queue-depth measurement. The resulting depth
   of four is an unsourced hidden parameter.
10. Branch-density and short-loop frontend charges search for the worst possible
    function-entry residue instead of using the final linked PC. That may be a
    conservative stress number, but it cannot measure whether block layout or
    relaxation improved the actual image.
11. `CoreBudget.charge` is computed and printed but is not added to
    `total_proxy_cycles` or workload rank. The gate vetoes growth in overflow
    counters, but code growth inside the hard limits is otherwise free and
    footprint shrinkage receives no rank credit. This disagrees with the
    documented claim that the I-side term is priced.

### Current-count reconciliation snapshot

On the current release build of `tests/golden/appliance/src/image.wr`:

```text
wrela dump --stage=cost:
    closure proxy cycles              1,459

image report:
    pre-link live-image proxy cycles 18,139
    pre-link function words          13,577  (executable_code_bytes 54,308 / 4)

final image layout:
    linked code words                13,841
    entry words                          20
    abort words                          30
    checkpoint words                      7
    all executable words             13,898
```

The current shipped `total_words` therefore omits 321 executable words on this
image, or 2.31% of final executable text. Of those, 264 are functions injected
during layout and 57 are fixed executable sections.

The current footprint reports `fetched_text_bytes=68,928`, while the raw final
entry+code+abort+checkpoint sections contain 55,592 bytes. Raw bytes and fetched
cache-line bytes are different metrics, but the 13,336-byte difference cannot
be interpreted as real cache-line rounding because it comes from hypothetical
per-function 64-byte placement rather than linked addresses.

The 18,139-cycle value is deterministic under the current model, but it is not
the cycle total of every executable word in the final image. Do not relock any
optimization result against it until the tasks below land.

### Cost task C1: make the linked executable stream the scoring input

This task precedes every transforming task in this plan.

Add a wide-only linked representation. Task 9 will later add variable-width
fragments to it:

```rust
struct LinkedProgram {
    sections: Vec<LinkedSection>,
    fns: BTreeMap<String, LinkedFn>,
    image_bytes: u64,
}

struct LinkedFn {
    key: String,
    section: SectionId,
    byte_address: u64,
    code: Vec<EmittedWord>,
    frame_size: u64,
}
```

Required behavior:

1. Perform every layout-time function injection before returning the program
   used by cost.
2. Preserve `EmittedWord` metadata while applying same-width relocations.
3. Represent entry, abort landings, checkpoint/vector code, and any other
   executable fixed section as synthetic functions with stable reserved keys.
4. Emit those fixed sections as `EmittedWord`, not raw `u32`, at their source.
   Serialization may discard metadata only after scoring and validation.
5. Record final byte addresses for every function and section.
6. Compute `total_words` from all final executable `EmittedWord`s. Do not count
   rodata, zero fill, or alignment padding as instructions; report those bytes
   separately.
7. The blob serialized from `LinkedProgram` must be byte-for-byte identical to
   the existing wide-only blob.
8. Give layout-injected and fixed-section functions stable origin-block
   partitions so measured-frequency resolution does not silently exclude them.

Suggested synthetic keys:

```text
__image_entry
__image_abort_fixed
__image_abort_value
__image_checkpoint_vector
```

Add an invariant:

```text
sum(final executable EmittedWord counts) * 4
    == sum(executable section payload bytes)
```

Padding between sections is reported but not treated as an instruction.

Acceptance:

- the appliance count reconciles to 13,898 executable words before any
  optimization changes the stream;
- layout-injected functions appear in cost terms and footprint;
- wide-only image bytes are unchanged;
- `cargo xtask verify` passes.

### Cost task C2: compute I-side footprint from final addresses

Replace the synthetic per-core repacking in `cost/footprint.rs`.

For each core:

1. collect the final address range of each hot emitted-word block that can run
   on that core;
2. convert the ranges to actual cache-line and page numbers;
3. union the actual line/page numbers;
4. compute set pressure from those real line numbers;
5. include shared code at its one real address on every core that executes it;
6. include synthetic entry/checkpoint/runtime sections on the cores that
   execute them.

Do not round each function to a cache line and do not shift a function to a
different address for per-core attribution.

Use the same final address for branch frontend terms. Replace the search over
all possible function-entry residues with the linked function's actual residue.
If the worst-residue calculation remains useful, expose it only as a stress
diagnostic. Hot/cold layout must be ranked on the frontend charge of the layout
it actually emits.

Keep these separately named metrics:

```text
executable_code_bytes  # instruction payload only
fetched_text_bytes     # union of actual cache lines
text_span_bytes        # optional: high address - low address, includes holes
image_bytes            # complete serialized image
```

Split the cycle result visibly:

```text
schedule_cycles
footprint_cycles
rank_cycles = schedule_cycles + footprint_cycles
```

For `flat`, compute footprint cycles from all statically scored blocks. For a
named workload, compute them from that workload's measured hot block set. Keep
the existing hard overflow veto in addition to, not instead of, the additive
charge. Use one definition in dumps, sweeps, attribution, and overall rank.

Delete or rename `packing_floor_lines` and `slack_lines` if they no longer have
a physical definition. Do not preserve them merely to keep old goldens stable.

Required tests:

- two small adjacent functions share a real cache line;
- repartitioning the same final words at the same addresses is footprint
  neutral;
- inserting one word changes only the lines/pages whose final ranges move;
- two cores use the same address for shared text;
- relocation relaxation moves later functions and recomputes their line/page
  identities.
- moving a branch across a 32-byte frontend boundary changes the actual-PC
  charge, while merely renaming or repartitioning unchanged addresses does not.
- increasing actual fetched lines cannot leave both `footprint_cycles` and the
  hard overflow counters unchanged when the applicable table term says those
  lines are priced.

Acceptance:

- the footprint's fetched lines can be reconstructed from linked addresses;
- no hypothetical function alignment remains;
- `cargo xtask verify` passes.

### Cost task C3: validate instruction metadata and repair dependencies

Add a small internal decoder/auditor for the AArch64 subset Wrela emits. It does
not need to disassemble text. For every final `EmittedWord`, validate:

- opcode family agrees with `CostRule`;
- encoded destination agrees with `dst`;
- every architecturally read GPR needed for scheduling appears in `srcs`;
- `MOVK` has its destination in `srcs`; `MOVZ` and `MOVN` do not;
- flag-writing and flag-reading instructions carry the right `FlagEffect`;
- load/store direction, width, and base agree with `mem` and `access_bytes`;
- direct calls declare the return/clobber output required by the current ABI;
- branch, call, and abort words are classified as control transfers.

Correct all `MOVK` emit sites, including runtime helper emitters:

```text
movz x9, ...  dst=x9 src=[]
movk x9, ...  dst=x9 src=[x9]
movk x9, ...  dst=x9 src=[x9]
```

Treat a returning call as a control boundary for following caller instructions.
The caller's post-call instructions may not issue before the call finishes.
The callee's dynamic work remains composed separately; do not paste the callee
schedule into the caller.

Add an exhaustive test that compiles the golden corpus, links each image, and
runs the metadata auditor over every emitted word. The existing source-text
search for selected encoder names may remain as a census, but it is not a
substitute for validating the actual words.

This correction will move cost goldens and optimization deltas. Land the model
fix and its relock in a dedicated commit. Cite old/new table and provenance
digests if the table text changes. Never alter thresholds to preserve an old
verdict.

### Cost task C4: remove the per-block compulsory-miss artifact

Separate pipeline scheduling state from memory-history state.

Neither flat source order nor a block-frequency vector is an execution trace,
so neither can justify a cross-block cache history. Use a removal-safe rank
policy for both flat and measured `s(block)`:

- stack and Flow frame homes enter as L1-resident;
- stores may forward to later loads within the same block;
- a symbolic static target may reuse a line within the same block;
- an unknown address gets the documented L1 latency floor in the ranking
  column and increments an `unresolved_mem` diagnostic;
- cache/TLB capacity is priced separately from final linked/placed footprints.

Calls do not evict cache lines. They end store-to-load forwarding knowledge for
addresses the callee may alias, and they form the control boundary from C3, but
known cache residency must not be cleared as though hardware flushed L1D.

Likewise, changing SP changes which symbolic stack frame an offset names; it
does not evict the old frame's physical cache lines. C5's memory identities
must express that distinction.

Delete the derived four-entry store-buffer capacity. Until a sourced queue
depth exists, model forwarding only for an exact same-target store that is not
separated from the load by a call, barrier, or possibly aliasing unknown store.
Do not invent eviction from the execution-port count.

Do not use an assumed L2/L3 compulsory miss to make deleting a load look more
profitable. If retaining the current pessimistic miss simulation is useful,
report it as a separate `stress_cycles` diagnostic; it must not be the sole
land-gate rank.

Extend cost terms with stable memory verdict counts:

```text
Mem level=l1_hit count=...
Mem level=forwarded count=...
Mem level=unresolved count=...
```

Required tests:

- two adjacent blocks loading the same frame line do not each pay a compulsory
  L2 access;
- deleting one hot frame load saves its L1-ranked cost, not an invented
  per-block compulsory miss;
- permuting mutually exclusive branch bodies does not change their memory
  charge;
- state-local register allocation's load/store deletion moves rule and memory
  counts exactly;
- frame coloring that only changes offsets cannot change instruction count.

### Cost task C5: give memory references symbolic provenance

`MemRef::Cold(base_register, immediate)` is not an address identity. The same
physical scratch register can hold unrelated absolute addresses, and different
registers can hold the same address.

Replace it with identities based on compiler knowledge:

```rust
enum MemTarget {
    Stack { function: FunctionId, offset: u64 },
    FlowFrame { function: FunctionId, offset: u64 },
    Static { symbol: SymbolId, offset: u64 },
    Mmio { device: DeviceId, offset: u64 },
    Unknown { site: SiteId },
}
```

Keep the actual base GPR separately for dependency validation.

After final placement, resolve static/Flow-frame targets to real line/page
numbers for footprint accounting. Unknown sites stay unknown; do not alias them
because they happened to use the same scratch register.

This task is required before using frame-byte, D-cache, or D-TLB changes as
profitability evidence for frame coloring or scalar replacement.

### Cost task C6: wire a usable measured workload into the real gate

Add an explicit source path for each named workload in
`bench/workloads.toml`, or an equally fail-closed repository mapping. The
optimization gate must compile and score that source for both sides; a library
unit test of `compare_overall` is not the gate.

Regenerate `boot-actors` with a measurement window:

1. finish boot;
2. clear counters;
3. enable Lane 2;
4. execute the workload scenario;
5. disable Lane 2;
6. only then run assertions, result formatting, and console output.

The committed vector should therefore describe production app/runtime work,
not the test harness that reports it.

Score the same linked function universe used to translate the measured IDs.
Every non-artifact measured key must resolve exactly. An unknown function or
out-of-range origin block is an error, not a maximum-function substitution in
the profitability gate. The maximum-function fallback may remain in diagnostic
composition for old sidecars, clearly labelled `unrankable`.

The actual enablement gate for frequency-dependent opts must:

- build baseline and candidate linked programs;
- resolve the same committed origin-key vector against both;
- score `sum(f(block) * s(block))` at every relevant residual-box point;
- add that workload's actual-address hot-footprint charge, not the flat
  all-block footprint;
- refuse changed frequency denominators;
- refuse lost key coverage;
- apply the existing per-core footprint and ordering vetoes;
- then perform the configured weighted relative rank.

Add one non-ignored integration test that proves changing a hot block changes
the named workload result by `frequency * block_delta`. Add another proving
that a cold-only block change moves words/footprint but not measured cycles.

Until this task lands:

- hot/cold layout is not rankable;
- proof-based bounds elimination is not rankable for product enablement;
- state-local register allocation may be evaluated only by flat static shape
  plus explicit load/store/frame metrics, not by claiming a measured product
  speedup.

### Cost task C7: make scope and totals explicit

Every cost dump and optimization table must state:

```text
scope=closure|linked-image
rank_cycles=...
stress_cycles=...          # only if retained
executable_words=...
executable_code_bytes=...
fetched_text_bytes=...
rodata_bytes=...
image_bytes=...
sync_frame_max_bytes=...
async_frame_total_bytes=...
```

Print per-function frame bytes and distinguish synchronous stack frames from
persistent Flow frames. For a placed image:

- synchronous frame pressure is a per-core maximum/high-water metric, not a
  sum of every function;
- persistent Flow-frame bytes are multiplied by the number of placed turn
  records that actually exist.

Make all optimization attribution helpers use `linked-image` for image roots.
A closure-only helper may remain, but its function name and output must say
`closure`; it may not feed an enable/park decision for a shipped image.

Add reconciliation invariants:

```text
sum(per-function executable words) == executable_words
sum(rule term counts)               == executable_words
linked executable payload bytes     == executable_words * 4
serialized section sizes            == report section sizes
```

### Cost task C8: remove false absolute-throughput precision

Until a whole-turn dynamic composition exists, replace:

```text
turns_per_sec
ms_per_turn_model
```

with a clearly named `max_entry_method_proxy_cycles`, without converting it to
throughput or time. Alternatively, implement whole-turn composition using the
measured block vector and include scheduler/runtime blocks before retaining
those projections.

The optimization ruler remains differential. A schedule proxy is not an
absolute performance prediction merely because it is divided by 2.4 GHz.

### Cost task C9: add optimization-specific counting oracles

Before enabling each transformation, add a linked-program oracle:

| Transformation | Required observed movement |
|---|---|
| state-local regalloc | exact reduction in hot `load`/`store` terms; barrier flush/reload terms remain |
| frame coloring | frame bytes or distinct frame lines fall; instruction count is unchanged unless an addressing form changes |
| scalar replacement | aggregate frame bytes/accesses fall; any added scalar copies are counted |
| immediate relaxation | final `mov_wide` word count falls and remaining `MOVK` dependencies validate |
| address relaxation | final `adrp`/`alu`/`mov_wide` mix and executable words match the selected encodings |
| hot/cold layout | instruction multiset stays equal except proven repair/inversion deltas; fetched lines use final addresses |
| bounds elimination | measured hot compare/branch terms fall; cold abort-path deletion is not presented as a hot cycle win |

For each oracle, assert both the exact term delta and the direction of
`rank_cycles`. A total-cycle assertion without a term-delta assertion is too
easy to satisfy through an unrelated runtime or coverage change.

## 3. Repository rules that apply to every step

1. Add no dependencies.
2. Prefer direct vectors, sorted vectors, `BTreeMap`, and `BTreeSet`.
3. Never use hash iteration order in a dump or allocation decision.
4. A malformed target, missing type, impossible frame home, invalid relocation,
   or inconsistent proof is a compile error. Do not silently fall back except
   where this plan explicitly says to retain the existing wide encoding.
5. Optimization profitability never controls correctness. The unoptimized and
   optimized programs must have identical observable behavior.
6. Add or extend a stable dump before making an analysis affect emission.
7. Run `cargo xtask verify` after every task below, including cost task C9 and
   tasks 5, 8, and 11 when closing the corresponding milestone.
8. Do not enable an `OptId` by default until the cost harness shows a win on its
   applicable workload. Experimental transforms may remain callable from unit
   tests or an explicit maintainer command.
9. Do not modify benchmark thresholds to make a regression pass. Threshold
   changes require a separately explained, reviewed result.

## 4. Stable identities and dump rules

All new dumps must obey these rules:

- Functions are printed in existing stable function order.
- Temps are `t0`, `t1`, and so on.
- Ordinary blocks are numbered by increasing original instruction start:
  `b0`, `b1`, and so on.
- Flow states retain their source IDs: `s0`, `s1`, and so on.
- Flow points use hierarchical IDs such as `s1.b0` and `s1.await`.
- Sets are sorted by numeric temp or block ID.
- Empty sets are `[]`, never omitted.
- Instruction ranges are half-open: `[start, end)`.
- Do not print pointer addresses, Rust debug maps, elapsed time, or allocation
  order.
- A later physical block permutation must not renumber the original CFG block.
  Keep an origin ID on emitted blocks.

Add the following dump stages to the existing dump/golden plumbing:

| Stage | Contents |
|---|---|
| `cfg` | CFG edges, block use/def, block live-in/live-out, per-instruction liveness, and FlowWir suspend-live sets |
| `frame` | per-temp storage class, per-state register assignment, frame homes, colored slots, and barrier flush/reload sets |
| `mwir-opt` | scalar-replacement decisions and explicit bounds-proof markers in optimized MWIR/FlowWir |
| `relax` | each relaxable site's stable ordinal, target, selected encoding, final width, and reason |

If the current golden harness expects a fixed list of stage files, extend that
list first. A missing expected dump must fail the golden test.

## 5. Task 1: centralize MWIR instruction facts

### Purpose

CFG liveness, DCE, register allocation, scalar replacement, and range analysis
must agree about which temps each instruction reads and writes. Today this logic
is partially duplicated.

### Files

- Add `crates/wrela-compiler/src/mwir_facts.rs`.
- Refactor the existing logic in `opts/mwir_opt.rs` and `regalloc.rs` to call it.
- Add unit tests beside the new module.

### Required API

Use an owned, sorted representation. The exact field names may follow local
style, but preserve these concepts:

```rust
pub(crate) struct InstFacts {
    pub uses: Vec<Temp>,
    pub defs: Vec<Temp>,
    pub address_escapes: Vec<Temp>,
    pub effects: Effects,
}

pub(crate) enum Effects {
    None,
    MayTrap,
    Observable,
}

pub(crate) fn inst_facts(inst: &MwirInst) -> InstFacts;
```

`uses` and `defs` are dataflow facts. `effects` is a separate removability fact.
A dead result does not make a call, store, send, check, or potentially trapping
operation removable.

Preserve the current mutation semantics:

- `SetField base, value`: uses `base` and `value`, and defines `base`.
- `IndexSet base, index, value`: uses all three, and defines `base`.
- `PlacedIndexSet` follows the same rule.
- Mutable map/slot operations that mutate a temp use and define that temp.
- A call uses its arguments, defines its return destination when present, and
  defines every writeback temp.
- Loads define their destination and use their address/base inputs.
- Stores use both address/base and value inputs.
- A projection uses the aggregate and defines the projected destination.

Sort and deduplicate every returned vector.

### Acceptance

- Existing optimized MWIR and assembly goldens do not change.
- Unit tests cover one instruction from every MWIR instruction family.
- A test explicitly distinguishes `defs` from side effects.
- `cargo xtask verify` passes.

## 6. Task 2: build and dump a stable synchronous CFG

### Files

- Add `crates/wrela-compiler/src/cfg.rs`.
- Add `crates/wrela-compiler/src/liveness.rs`.
- Add `cfg` to the compiler dump enum/CLI and golden harness.

### CFG construction

Create blocks from a function's MWIR instruction vector.

Leaders are:

1. instruction `0`, when the function is non-empty;
2. every valid local jump target;
3. the instruction after a jump, conditional jump, return, or abort, when that
   instruction exists.

Each block owns a half-open instruction range. Assign `BlockId`s by increasing
range start.

Successors are:

- unconditional jump: target only;
- conditional jump: target, then fallthrough, deduplicated and sorted;
- return, abort, and assertion failure: none;
- other final instruction: fallthrough when one exists.

A jump target equal to `instructions.len()` means function exit. A target larger
than that is an error. Empty and unreachable blocks must be represented
deterministically; do not discard unreachable blocks in this analysis.

Store predecessors as sorted vectors computed from successors.

### Block use/def

For each block, scan forward:

```text
for each instruction:
    block.use += instruction.uses not already in block.def
    block.def += instruction.defs
```

### Liveness

Compute the standard backward fixed point:

```text
live_out[B] = union(live_in[S] for S in successors[B])
live_in[B]  = use[B] union (live_out[B] - def[B])
```

Initialize all sets empty. Iterate blocks in reverse numeric order until a full
pass makes no change. This is intentionally simple.

Then scan each block backward to compute liveness immediately before and after
each instruction:

```text
after[I]  = current
before[I] = uses[I] union (after[I] - defs[I])
current   = before[I]
```

Parameters and the receiver are definitions available at entry. Do not force
them live for the whole function. A returned temp is live because `Return` uses
it.

### Tricky example: a diamond

Input:

```text
0: t0 = const false
1: jump_if_false t0 -> 4
2: t1 = const 10
3: jump -> 5
4: t1 = const 20
5: return t1
```

Expected block summary:

```text
b0 [0,2) succ=[b1,b2] use=[]   def=[t0] live_in=[]   live_out=[]
b1 [2,4) succ=[b3]    use=[]   def=[t1] live_in=[]   live_out=[t1]
b2 [4,5) succ=[b3]    use=[]   def=[t1] live_in=[]   live_out=[t1]
b3 [5,6) succ=[]      use=[t1] def=[]   live_in=[t1] live_out=[]
```

It is correct for `t1` not to be live out of `b0`: every path defines it before
the use. If the implementation reports `t1` live out of `b0`, its kill handling
is wrong.

### Required tests

Add focused golden cases:

- `check-cfg-straight`
- `check-cfg-diamond`
- `check-cfg-loop`
- `check-cfg-unreachable`
- `err-cfg-invalid-target` as a Rust unit test if invalid MWIR cannot be
  constructed from Wrela source

The `cfg` dump must include both block and per-instruction sets.

### Acceptance

- Re-running the same dump is byte-for-byte identical.
- Existing DCE can use the new facts without changing behavior.
- `cargo xtask verify` passes.

## 7. Task 3: add FlowWir CFG and suspend liveness

### Purpose

Ordinary liveness is insufficient at an `await`. A temp used after resumption
needs durable storage; a temp used only to form the request does not.

### Flow graph

Do not flatten FlowWir through codegen to analyze it. Build the graph from
`FlowWirFn`, preserving state IDs.

Within a state, build ordinary blocks from the state's MWIR-local jumps. Add a
transition point after the last local block. Model Flow transitions as follows:

- `Return` and `Abort`: no successors.
- `Jump state`: successor is that state's entry.
- `Branch`: successors are the two state entries.
- `Await`: successor is a synthetic resume-definition point, followed by the
  resume state's entry.

The synthetic point defines `result_temp`. This matters: the await result does
not exist before suspension and therefore must never be saved across the
suspension merely because the resume state reads it.

The await transition uses every temp needed to form the request, including its
target, arguments, and take operands.

Treat transition operands as normal uses:

- `Return value` uses `value`, when present.
- `Branch condition` uses `condition`.
- `Await` uses its request operands.
- `Jump` and `Abort` have no temp uses unless their concrete FlowWir variants
  contain an operand, in which case that operand must be listed explicitly.

Run the same fixed-point algorithm over Flow points.

Define:

```text
suspend_live(await) = live_out at the await transition
```

The synthetic result definition should already remove `result_temp`. Add an
assertion that `result_temp` is absent from `suspend_live`.

### Tricky example: request values versus durable values

```text
state s0:
    t0 = const 40
    t1 = make_target ...
    await target=t1 args=[t0] result=t2 resume=s1

state s1:
    t3 = add t0, t2
    return t3
```

Expected facts:

```text
s0.await uses=[t0,t1]
s0.await suspend_live=[t0]
s1.resume_def defs=[t2]
s1.entry live_in=[t0,t2]
```

`t1` is needed to issue the await but not after it, so it is not durable.
`t2` is created by the resume and is not durable across this await. `t0` is
durable because the continuation reads it.

Add a second example where a taken resource is not read after resume. It must
not appear in `suspend_live`. If lowering permits reading a taken resource after
resume, the existing ownership checker should reject the source before this
analysis.

### Dump

Extend `cfg` with:

```text
flow function ...
  s0.b0 ...
  s0.await succ=[s1.resume_def] uses=[t0,t1] live_out=[t0]
  s1.resume_def defs=[t2] live_out=[t0,t2]
  suspend s0 -> s1 save=[t0] result=t2
```

### Required tests

- await result used immediately after resume;
- request-only argument;
- value live across two separate awaits;
- branch after resume;
- state loop;
- unreachable Flow state;
- malformed resume state and malformed result temp fail closed.

Add `check-flow-suspend-live` and `check-flow-multi-suspend` goldens.

### Acceptance

- This task changes no emitted code or frame size.
- `cargo xtask verify` passes.

## 8. Task 4: introduce an explicit frame plan

### Purpose

The current synchronous assignment treats a register-resident temp as having no
frame home. That is not valid for FlowWir: a temp can need a durable frame home
across an await and still benefit from a register cache while one state runs.

Do not pass a synchronous `Assignment` directly to `build_frame_flow`.

### Representation

Add a frame-planning structure, preferably in a new
`crates/wrela-compiler/src/frame_plan.rs`:

```rust
enum Home {
    None,                   // register-only in every region where it exists
    Frame { slot: SlotId }, // durable or spilled
    Pinned { offset: u32 }, // ABI-defined location
}

struct StateAssignment {
    state: StateId,
    temp_regs: Vec<Option<PhysReg>>,
    live_in_loads: Vec<Temp>,
    exit_flushes: Vec<Temp>,
}

struct FramePlan {
    homes: Vec<Home>,
    states: Vec<StateAssignment>,
    slots: Vec<FrameSlot>,
}
```

Use vectors indexed by stable IDs where possible.

Keep ABI regions separate from colorable temp slots:

- saved link register;
- receiver/mutable parameter/return-pointer homes;
- reply staging;
- entropy/checkpoint scratch;
- fixed turn-record fields and other runtime ABI prefixes.

### Storage classes

Classify each temp:

- `pinned`: ABI or address identity requires a fixed home;
- `persistent`: present in any suspend-live set;
- `boundary-live`: live across a non-suspending state `Jump` or `Branch`;
- `resume-result`: defined by an await resume edge;
- `escaped`: its address or aggregate identity escapes;
- `state-local`: never live across a state boundary;
- `state-spill`: local to a state but not assigned a register.

`persistent`, `boundary-live`, and `resume-result` temps get frame homes in the
first implementation. A resume result needs a home because the runtime delivers
it before the resumed state's register cache exists. A later change may move it
directly from reply staging to a register, but that is not part of this plan.

At this task, keep the existing uncolored offsets. The dump is the deliverable.

### Dump

Example:

```text
temp t0 class=persistent home=slot3 states=[s0:x20,s1:x19]
temp t1 class=state-local home=none states=[s0:x21]
temp t2 class=resume-result home=slot4 states=[s1:x20]
state s0 entry_load=[] await_flush=[t0]
state s1 entry_load=[t0] return_flush=[]
```

### Acceptance

- No assembly change.
- Every temp used by emission has either a home or a register in that state.
- Every suspend-live temp has a durable home.
- Dump validation rejects a persistent temp with `Home::None`.
- `cargo xtask verify` passes.

## 9. Task 5: implement state-local register allocation

### Core rule

A register assignment is valid only while executing one Flow state. No register
value survives `emit_park_and_return`, and no successor state may assume the
predecessor's register assignment.

For the first correct implementation, every state boundary is a register
allocation barrier:

- At state entry, raw-load each assigned live-in that has a frame home.
- At `Await`, raw-store each dirty temp in `suspend_live`.
- At a non-suspending `Jump` or `Branch`, raw-store each dirty successor
  live-in with a home.
- At `Return` or `Abort`, do not flush dead values.
- The successor independently reloads its assigned live-ins.

These stores and loads may later be coalesced, but not in this task.

### Refactor required in codegen

The emitter currently maps a virtual frame offset to a resident register and
can omit the physical slot. Split those concepts:

1. `FramePlan` owns the physical home, if any.
2. The current state's `StateAssignment` owns the register cache.
3. Normal `load_slot`/`store_slot` may use the register cache.
4. Barrier loads/stores use explicit raw helpers that bypass the cache.
5. Track dirty assigned temps. A definition marks dirty; a barrier store clears
   dirty.
6. Before first use at state entry, preload assigned live-ins. Do not preload a
   temp that is definitely defined before use in the state.

For a state-local register-only temp, retain the existing virtual location
mechanism or introduce an explicit location ID, but it must never be passed to
a raw frame load/store.

### Allocation input

Refactor the existing emission probe so it can collect slot accesses and call
clobbers for one state. Use:

- the state's internal CFG liveness;
- exact probed instruction positions;
- existing scalar eligibility rules;
- existing ABI register exclusions;
- local backedges from that state's MWIR jumps.

Do not use physical state order or later block layout order to build intervals.
Use stable CFG/instruction identities.

Call boundaries retain the existing clobber restrictions. Await is stronger
than a call: all register cache state is discarded.

### Tricky example: same durable temp, different registers

```text
s0: t0 is computed in x20; await; save t0 to slot2
s1: load slot2 into x19; use t0; branch s2
s2: load slot2 into x21; return t0
```

This is legal. There is no requirement that `t0` use one physical register in
all states.

This is not legal:

```text
s0: t0 is in x20; await
s1: use x20 without a load
```

Add an emitter validation mode that catches the second case.

### Measurement and acceptance

- Add `check-flow-state-regalloc` and a high-pressure spill case.
- Assembly golden must show the await flush before parking and resume load after
  dispatch.
- Cost task C9 must show the exact linked hot-load/hot-store delta. Do not
  compare against the old per-block compulsory-memory score.
- The old no-register path remains available for differential tests.
- Differential execution covers every new golden.
- Frame size must not increase on the targeted Flow cases.
- Enablement requires a non-regressing measured result; otherwise keep the
  implementation behind an experimental `OptId`.
- Run `cargo xtask verify`.

## 10. Task 6: color frame slots

### Scope

Color only compiler-owned temp homes. Never color or overlap fixed ABI regions.

Start with exact `(size, alignment)` classes. Do not attempt sub-slot packing or
mixed-size reuse in version 1.

### Interference

Build interference from per-instruction liveness:

1. For each instruction definition `d`, add interference between `d` and every
   temp in `live_after`.
2. Add pairwise interference for every set of temps simultaneously live at a
   function/state entry. This covers live-ins that have no definition in the
   region.
3. At state/await barriers, all simultaneously durable values interfere.
4. Pinned and address-escaped temps are not color candidates.
5. Temps with different exact size/alignment classes do not share a slot in v1.

Use a symmetric matrix or sorted adjacency vectors. The expected temp counts are
small enough that simplicity is preferred.

### Deterministic first-fit

Within each size/alignment class:

1. sort temps by numeric `TempId`;
2. inspect existing candidate slots by increasing `SlotId`;
3. place the temp in the first slot whose occupants do not interfere;
4. otherwise create a new slot.

After coloring, lay out slots by:

1. decreasing alignment;
2. decreasing size;
3. increasing `SlotId`.

Insert explicit padding and validate every final offset.

### Tricky example: mutually exclusive values

```text
if cond:
    t1 = make_scalar()
    consume(t1)
else:
    t2 = make_scalar()
    consume(t2)
return
```

If `t1` and `t2` have the same size/alignment and are never simultaneously
live, they may occupy one slot:

```text
slot0 offset=32 size=8 align=8 occupants=[t1,t2]
```

By contrast, a loop-carried `t3` that is live on the backedge must interfere
with any value simultaneously live at that point.

### Validation

After layout:

- every home offset is aligned;
- no slot overlaps an ABI region;
- interfering temps never share a slot;
- every load/store width fits its slot;
- frame size respects existing architectural limits.

Add `check-frame-color-diamond`, `check-frame-color-loop`, and Flow suspension
cases. Include the slot occupant list in `frame`.

### Acceptance

- No frame grows solely because coloring is enabled.
- At least one golden demonstrates actual reuse.
- The linked cost report's persistent Flow-frame bytes or distinct frame-line
  count falls on that golden, even if executable words are unchanged.
- `cargo xtask verify` passes.

## 11. Task 7: add scalar-replacement analysis and dump

### Version 1 candidates

Only scalarize structs and tuples whose complete use graph is visible in MWIR.
Exclude arrays and enums initially.

A candidate aggregate and every aggregate copy connected to it may only be used
by:

- aggregate construction;
- same-type aggregate copy;
- constant-field projection;
- constant-field mutation.

Reject with a stable reason if it is:

- passed to or returned from a call;
- returned from the function;
- used by send, await, group, MMIO, pointer, or raw memory operations;
- dynamically indexed;
- used in a way that exposes its address or aggregate identity;
- an ABI-pinned receiver, parameter, writeback, or return location;
- an unsupported nested aggregate.

Nested structs/tuples may be flattened recursively into leaf paths such as
`.0.1`, but cap recursion at the language's existing type recursion limits.

### Analysis-only dump

Before rewriting, add output like:

```text
sroa t4 candidate leaves=[.0:i64,.1:bool]
sroa t8 rejected reason=passed-to-call at=17
sroa t9 rejected reason=dynamic-index at=22
```

This task must not change MWIR.

### Acceptance

- Unit tests cover every rejection reason.
- Candidate discovery is deterministic.
- `cargo xtask verify` passes.

## 12. Task 8: rewrite scalar-replacement candidates

### Rewrite

Allocate one fresh scalar temp per leaf path. Rewrite:

```text
t0 = make_struct [t1, t2]
t3 = project t0.0
set_field t0.1 = t4
```

as:

```text
t0.0 = copy t1
t0.1 = copy t2
t3   = copy t0.0
t0.1 = copy t4
```

The names above are dump notation; actual leaves receive normal fresh numeric
temp IDs and types.

Because MWIR temps are mutable, both arms of a branch may define the same leaf
temp. Ordinary CFG liveness handles the join; do not invent SSA phi nodes.

### Tricky example: aggregate copy overlap

An aggregate copy must behave like a parallel copy. If source and destination
leaf sets can overlap, this sequential rewrite is wrong:

```text
dst.0 = src.0
dst.1 = src.1
```

Use fresh scalar scratch temps:

```text
scratch0 = src.0
scratch1 = src.1
dst.0 = scratch0
dst.1 = scratch1
```

For provably disjoint source and destination leaves, direct copies are allowed.
Keep the conservative scratch form initially if alias/disjointness is unclear.

Run ordinary MWIR cleanup after rewriting so redundant scalar copies may be
removed. Then rerun liveness, register allocation, and frame coloring.

### Required tests

- construct/project elimination;
- field update;
- aggregate copy;
- overlapping/cyclic-looking copy;
- branch definitions and join;
- nested tuple/struct;
- every rejection class remains unmodified;
- synchronous and FlowWir cases.

Add `check-sroa-struct`, `check-sroa-copy`, and `check-sroa-flow`.

### Acceptance

- Optimized `mwir-opt` contains no aggregate instructions for accepted
  candidates.
- Differential execution matches the unoptimized program.
- At least one case has a smaller frame or fewer frame accesses.
- `cargo xtask verify`.

## 13. Task 9: introduce a late-link fragment representation

### Purpose

Existing relocation patching assumes fixed-width instruction sequences,
including four-word immediate materialization. Shrinking one sequence changes
all later function bases and relocation indices. Do not mutate the flat word
vector in place while relying on old indices.

Cost task C1 must already have made the wide-only `LinkedProgram` the single
input to scoring, footprint, reporting, and serialization. Extend that
representation here; do not create a second linker used only by relaxation.

### Representation

Introduce a pre-link fragment stream:

```rust
enum Fragment {
    Fixed(EmittedWord),
    Relax(RelaxSite),
}

struct RelaxSite {
    ordinal: u32,       // stable within the containing function
    kind: RelaxKind,
    target: RelaxTarget,
    selected: Encoding,
    frozen_wide: bool,
}
```

The stable ordinal is assigned in source emission order. It does not change
when widths change.

`RelaxTarget` must be symbolic: function, rodata item, runtime field, fixed
machine address, or immediate value. Do not store a stale word index as the
target.

Refactor the linker/layout path into these conceptual phases:

1. inject runtime fragments;
2. choose provisional fragment widths;
3. assign function and section bases;
4. select/validate relax encodings;
5. repeat if widths changed;
6. flatten once;
7. resolve calls, branches, and remaining relocations using final indices;
8. validate and serialize.

The cost/report path must score final linked `EmittedWord`s, not maximal
placeholders. If necessary, return a `LinkedProgram` beside `ImageLayout`.

### Analysis-only first commit

Initially, every `RelaxSite` selects the existing wide encoding. The generated
blob and cost must remain byte-for-byte unchanged. Add `relax` dump output.

### Acceptance

- Existing images are byte-for-byte unchanged in the wide-only mode.
- There is no relocation index into a mutable pre-flattened word vector.
- `cargo xtask verify` passes.

## 14. Task 10: relax immediate materialization

### Encoding choice

For a known 64-bit value, select the shortest existing AArch64 materialization
that the emitter already knows how to encode:

1. one `MOVZ`, `MOVN`, or logical-immediate instruction when legal;
2. otherwise a base `MOVZ`/`MOVN` plus only the required `MOVK` lanes;
3. otherwise the existing four-word form.

Tie-break deterministically using a documented fixed preference order. Do not
pick based on Rust enum order.

Start with value-only relocations such as turn ID, stride, vector number, and
other true constants. Do not treat an address as a value-only relocation.

### Required tests

Test zero, all ones, one nonzero 16-bit lane, sparse two-lane values, values for
which `MOVN` wins, and the four-lane worst case.

The dump should read:

```text
relax fn=foo site=3 kind=imm value=0x0000000000000040
  encoding=movz width=1 old_width=4 saved_words=3
```

### Acceptance

- Decode or execute every selected sequence in a unit test.
- Every retained `MOVK` passes cost task C3's destination read-dependency
  validation.
- Linked cost counts the selected width.
- `cargo xtask verify` passes.

## 15. Task 11: relax addresses to PC-relative forms

### Encoding order

For address-valued sites, try:

1. `ADR` when the final signed byte displacement fits its architectural range
   and alignment requirements;
2. `ADRP` plus `ADD` when both instructions are implemented and page
   displacement/range checks pass;
3. minimal absolute materialization from task 10;
4. existing four-word absolute materialization.

Do not add `ADRP` in this task if the emitter/decoder/validator does not already
support it. It is acceptable for version 1 to use only `ADR` and absolute
materialization.

### Fixed-point algorithm

Use this terminating algorithm:

1. Start every site wide and unfrozen.
2. Lay out all fragments using current widths.
3. Select the shortest valid encoding at each unfrozen site.
4. Rebuild all bases and indices.
5. Revalidate every short site against the rebuilt layout.
6. If a short site is now invalid, expand it to wide and set
   `frozen_wide = true`.
7. Repeat until a full iteration changes no width.

A frozen site never shrinks again during that link. This guarantees termination
even when shrinking code changes the distance to a fixed runtime-data region in
an inconvenient direction.

Patch ordinary calls and branches only after the final iteration.

Cap iterations at `2 * relax_site_count + 1`; exceeding the cap is an internal
compiler error because the monotone freeze rule should make it impossible.

### Tricky example: shrink changes another displacement

```text
function A:
    site 0 -> near rodata
    site 1 -> fixed RTDATA address
```

Shrinking site 0 moves site 1 earlier. Its displacement to fixed RTDATA grows.
Therefore it is incorrect to decide both sites once from the maximal layout and
blindly keep both decisions. The rebuild/revalidate/freeze loop handles this.

### Validation and tests

- range boundary exactly in and one byte/instruction out;
- forward and backward targets;
- target after a site that shrinks;
- two sites whose decisions change provisional addresses;
- multiple functions where the first function shrinks;
- fixed machine address and rodata target;
- every old four-word relocation validator updated to validate the selected
  encoding instead of assuming width four.

Dump a reason for wide selections:

```text
encoding=wide width=4 reason=adr-out-of-range frozen=true
```

### Acceptance

- All final displacements are recomputed from final addresses.
- The linked blob disassembles/executes identically.
- At least one product image shrinks.
- `cargo xtask verify`.

## 16. Task 12: re-ask hot/cold block layout

### Preconditions

Do not start this task until:

1. register allocation is based on stable CFG identity rather than physical
   emitted order;
2. reordering a legal CFG permutation leaves temp-to-register choices
   unchanged, except for irrelevant block labels;
3. emitted blocks carry exact origin IDs through repair blocks;
4. the classifier and the emitted partition bridge refer to the same CFG.
5. cost tasks C1, C2, and C6 are complete, so the layout is scored at final
   addresses under a genuinely measured workload.

The existing experiment increased code/word footprint because reordering
changed register allocation and introduced repair/spill work. The preconditions
are the point of re-asking it later.

### Initial algorithm

Keep it intentionally modest:

1. classify blocks from the existing profile/cost evidence;
2. retain source order within hot, unknown, and cold classes;
3. order the classes hot, unknown, cold;
4. choose a legal fallthrough by inverting a conditional only when doing so
   removes a repair jump;
5. insert repair jumps only when the new adjacency changes required control
   flow;
6. keep original block IDs in dumps and cost attribution.

Do not add frequency heuristics, traces, or a new graph algorithm in version 1.

### Required tests

- diamond with either arm cold;
- loop with cold exit;
- unreachable cold block;
- conditional inversion;
- repair jump insertion;
- exact classifier-to-emitted-block mapping;
- compare register assignments before and after a legal permutation.

### Profitability gate

Keep block layout experimental unless the applicable measured workload shows:

- no increase in spill words;
- no increase in total shipped words;
- no increase in total measured cycles;
- correct coverage/partition attribution;
- exact preservation of the non-repair instruction multiset;
- no budget or golden regressions.

Flat micro-cost improvement alone is insufficient for a frequency-dependent
layout transform. Prefer the overall/product measured workload.

### Acceptance

- `cargo xtask verify` passes.
- If the gate does not pass, leave the transform parked and record the measured
  result; do not tune thresholds.

## 17. Task 13: add range analysis and proof dumps

### Purpose

The existing literal-only bounds elision is safe but weak. Replace syntax-local
guessing with an explicit proof at the exact indexed instruction.

### Lattice

For integer temps:

```rust
enum Range {
    Bottom,
    Interval { lo: i128, hi: i128 },
    Top,
}
```

Clamp intervals to the declared integer type's bounds. Join is the convex hull.
Use checked host arithmetic while computing endpoints; overflow yields `Top`,
not wraparound.

### Transfers in version 1

Implement:

- integer constants;
- copies;
- add/sub by a constant when endpoint arithmetic is representable;
- narrowing/widening conversions when their semantics are known;
- unsigned type lower bound of zero;
- comparison refinement on branch edges for `<`, `<=`, `>`, `>=`, and `==`.

Optional only after those work: multiplication by a nonnegative constant.

Only refine a branch when its condition has one unambiguous reaching definition
that is the relevant comparison and neither comparison operand is redefined
between that comparison and the branch. Otherwise propagate the unrefined
incoming range. Do not guess through an ambiguous join.

For loops, iterate to a fixed point and widen an expanding bound to the integer
type bound after two expansions. Record widening in the dump. Do not implement
relational domains in version 1.

At an await/resume boundary, initially retain only constants and facts that can
be justified from immutable persistent values. Clearing other facts is safe and
acceptable.

### Proof condition

For an index with array length `N`, eliminate the check only if the range at
that exact instruction proves:

```text
0 <= index.lo
index.hi < N
```

For an unsigned index, the type proves the first condition. For a signed index,
an upper-bound comparison alone is insufficient.

### Tricky examples

Unsigned branch:

```text
if i < 16 {
    use a[i]       // proven when len(a) == 16 and i is unsigned
}
```

Signed branch:

```text
if i < 16 {
    use a[i]       // not proven: i may be negative
}
```

Signed two-sided branch:

```text
if 0 <= i && i < 16 {
    use a[i]       // proven
}
```

Join loses precision:

```text
if cond { i = 0 } else { i = 100 }
use a[i]           // range [0,100], not proven for len 16
```

This conservative loss is acceptable.

### Dump

Extend `mwir-opt` or add the proof section before changing emission:

```text
at=17 t3 range=[0,15]
bounds base=t8 index=t3 len=16 result=proven
at=29 t6 range=top
bounds base=t9 index=t6 len=16 result=unknown reason=upper-bound
```

### Acceptance

- No emitted code changes in this task.
- Unit tests cover joins, loops, signedness, overflow, and await barriers.
- `cargo xtask verify` passes.

## 18. Task 14: make bounds elimination proof-carrying

### Representation

Do not use a side table keyed only by a mutable instruction index. Attach the
decision to the optimized IR.

Use either:

- a `BoundsCheck` field on indexed MWIR instructions with
  `Required`/`Proven { len }`; or
- explicit checked and proven instruction variants.

Prefer one shared field if it avoids duplicating every indexed instruction
variant.

The code generator may omit a check only for `Proven`, and validation must
confirm that:

- the recorded length matches the indexed type/layout;
- the index type matches the proof;
- no later rewrite changed the base or index without clearing the marker.

Every transform that rewrites a proven indexed instruction must either preserve
and revalidate the exact operands or reset it to `Required`.

Keep the current literal fast path, but make it create the same proof marker
through the range-analysis API instead of bypassing it in both lowerers.

### Required tests

- all task 13 examples;
- exact lower and upper boundaries;
- zero-length aggregate;
- signed negative constant;
- loop induction variable;
- arithmetic overflow becomes unknown;
- proof cleared after operand rewrite;
- both synchronous MWIR and FlowWir;
- differential execution for checked and proven forms.

### Profitability gate

Re-measure the existing parked `BoundsElide` optimization after the proof pass.
Enable it only after cost task C6, and only when it wins the applicable
shipped/product workload from hot compare/branch removal. Cold abort-path
deletion may support a word/footprint win but must not be described as a hot
cycle win. If it is correct but neutral, keep it available but parked.

### Acceptance

- `mwir-opt` visibly distinguishes checked and proven indexing.
- Codegen has no independent “looks in range” heuristic.
- Invalid proof metadata fails closed.
- `cargo xtask verify` passes.

## 19. Final integrated validation

After all tasks:

1. Run `cargo xtask verify`.
2. Optionally run `cargo xtask verify-deep` for the expensive whole-corpus
   maintainer diagnostics.
3. Run focused optimized versus unoptimized differential execution for every
   new golden.
4. Compare product workload cycles, shipped words, spill words, frame bytes,
   and image bytes.
5. Run `cargo xtask fuzz all` separately if time permits. Any finding must
   become a permanent unit or golden test before its fix.
6. Confirm all reported image metrics come from the same final
   `LinkedProgram`, and that no closure-only number appears in an image
   enablement decision.

The final report must include a table:

| Optimization | rank cycles before/after | executable words before/after | frame before/after | fetched text before/after | enabled? |
|---|---:|---:|---:|---:|---|
| Flow state-local regalloc | | | | | |
| Frame coloring | | | | | |
| Scalar replacement | | | | | |
| Immediate relaxation | | | | | |
| Address relaxation | | | | | |
| Hot/cold layout | | | | | |
| Proof bounds elimination | | | | | |

“Correct but not enabled” is an acceptable result. A transform that loses the
product gate should remain parked with its dumps and tests intact.

## 20. Suggested commit sequence

Use one commit for each row unless a row is purely mechanical and inseparable
from its neighbor:

1. C1 wide-only linked scoring artifact and executable-section metadata;
2. C2 actual-address I-side footprint;
3. C3 instruction metadata audit, `MOVK` dependencies, and call control;
4. C4 block-memory rank correction and memory verdict dump;
5. C5 symbolic memory provenance;
6. C6 measured workload window and real overall gate;
7. C7 reconciled scope/count/frame reporting;
8. C8 throughput-label correction;
9. C9 linked optimization counting oracles;
10. central MWIR instruction facts;
11. synchronous CFG and liveness dump;
12. Flow CFG and suspend-liveness dump;
13. frame-plan dump with no emission change;
14. state-local FlowWir register allocation;
15. frame-slot coloring;
16. scalar-replacement candidate dump;
17. scalar-replacement rewrite;
18. variable-width linker fragments and relaxation dump;
19. immediate relaxation;
20. address relaxation;
21. CFG-stable block-layout re-evaluation;
22. range-analysis proof dump;
23. proof-carrying bounds elimination;
24. integrated measurement report and enable/park decisions.

If a commit cannot pass `cargo xtask verify`, stop and fix it before starting
the next row. Do not carry a known failure forward.

## 21. Integrated implementation report

The shared linked representation, final-address scoring, symbolic memory
provenance, stable dataflow dumps, and monotone late relaxation are integrated.
Transforms that do not yet have a passing applicable workload gate remain
callable for analysis/tests but are deliberately not enabled by default.

The pinned rows below use the final linked `cost-product-actors` image and
isolate one option from the current release set. Frame values are shown as
`synchronous maximum / persistent async total`.

| Optimization | rank cycles before/after | executable words before/after | frame before/after | fetched text before/after | enabled? |
|---|---:|---:|---:|---:|---|
| Flow state-local regalloc | 10,844 / 10,682 | 14,396 / 14,245 | 1,328/944 / 1,328/944 | 57,728 / 57,024 | yes; exact linked gate passes |
| Frame coloring | 10,682 / 10,787 | 14,245 / 14,334 | 1,328/944 / 1,328/272 | 57,024 / 57,472 | no; frame falls but rank and words regress |
| Scalar replacement | 10,682 / 10,682 on the named product | 14,245 / 14,245 | 1,328/944 / 1,328/944 | 57,024 / 57,024 | no; full flat-corpus veto remains |
| Immediate relaxation (`NarrowImm` attribution) | 18,588 / 10,682 | 22,429 / 14,245 | unchanged | 89,856 / 57,024 | yes |
| Address relaxation | linked wide / final | 14,723 / 14,245 | unchanged | final 57,024 | yes; 478 address words saved |
| Hot/cold layout | 10,682 / 10,682 | 14,245 / 14,245 | unchanged | 57,024 / 57,024 | no; current measured ordering is identity |
| Proof bounds elimination | 10,821 / 10,682 | 14,606 / 14,245 | unchanged | 58,560 / 57,024 | yes; exact linked gate passes |

The exact workload gate resolves `1512/1512` `boot-actors` observations on both
sides. The full residual/corpus gate passes for `BoundsElide` and
`FlowStateRegs`; SROA remains parked, and frame coloring remains parked despite
its persistent-frame reduction.

The release linked oracle and relaxation dump are the source of truth. On the
appliance, final linked output is 13,533 executable words, 54,132 executable
bytes, 54,208 fetched text bytes, and 560 rodata bytes. On
`cost-product-actors`, the relaxation dump records 489 address sites saving 478
words and immediate sites saving 21 words in total.
