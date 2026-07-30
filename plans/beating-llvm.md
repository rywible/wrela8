# Beating LLVM: the structural advantage inventory

**Status: BRAINSTORM (2026-07-29).** Not a plan — a capability inventory
and an argument. Companion to [opts-ladder.md](opts-ladder.md): the ladder
lists opts to build on the current backend; this document asks the
different question of *where the ceiling is* and why it might be above
LLVM's. Nothing here activates without its own plan, its own freezes, and
the M20 ruler to score it.

## The frame: why hand-written assembly beats compilers

"As good as ffmpeg's hand asm" is a real target only if you can say what
hand asm actually has that compiler output doesn't. It is a short list —
about ten items — and the useful exercise is marking which ones wrela
**structurally** removes (not "could optimize better," but *the reason
does not apply*).

| # | Why hand asm wins | wrela |
| --- | --- | --- |
| 1 | Human knows the aliasing; compiler must prove it | **removed** — exclusivity is a type-system fact |
| 2 | Human ignores the ABI (owns all callers) | **removed** — no external linkage, ever |
| 3 | Human knows alignment; compiler handles unaligned | **removed** — compiler owns layout |
| 4 | Human knows the trip count/shape; compiler emits remainder + dispatch | **removed** — `@budget` bounds, comptime sizes, measured trips |
| 5 | Human allocates registers globally for the kernel | **removed** — no ABI + whole program ⇒ interprocedural RA |
| 6 | Human uses instructions the compiler won't emit | **reducible** — single target, no feature guards; someone still writes the patterns |
| 7 | Human schedules for the exact microarchitecture | **mostly moot** — A76 is OoO with a 128-entry window; hardware reschedules |
| 8 | Human knows algorithm-level invariants ("always < 256") | **partly removed** — sized types + checked arithmetic are a range oracle |
| 9 | Human accepts unmaintainable, uncompilable-in-1s code | **not removed** — compile time is a product number here too |
| 10 | Human chose a better algorithm / shuffle sequence | **not removed** — no compiler derives this |

**Seven of ten removed or reduced.** That is the case, and it is not
aspirational — each row below is a mechanism, not a hope. The two that
survive (9, 10) draw the honest boundary, stated at the end.

## A. The no-ABI family — LLVM cannot do these at all

The single most under-exploited fact in the project: **no function in a
sealed image is callable from outside it.** AAPCS64 exists so strangers can
call your code. wrela has no strangers. LLVM approximates this with
`internal` + `fastcc` + LTO, but visibility rules, symbol interposition,
and plugin boundaries keep it partial; wrela has none of those.

1. **Interprocedural register allocation.** One allocation problem over the
   whole program, custom convention per function, computed globally. This
   is the legitimate form of the "one function per vCPU" instinct — you get
   cross-call register residency *without* fusing code, so live-range
   firewalls survive.
2. **No callee-saved discipline.** The caller knows exactly which registers
   the callee clobbers (whole program), so conservative save/restore
   disappears entirely rather than being minimized.
3. **Frameless functions.** A function whose values all fit in registers
   needs no stack frame: prologue, epilogue, and `sub sp` all vanish. On a
   spill-everything baseline this is enormous.
4. **Arbitrary-arity register passing.** Twenty live values in registers
   across a call if that is what the allocator wants. No x0–x7 limit, no
   x8 indirect-result convention, no struct-return dance.
5. **Multiple return values in registers**, no hidden out-pointer.
6. **Universal tail calls.** No ABI means every tail-position call is a
   jump. Unconditionally, not as an optimization that sometimes applies.
7. **Argument-specialized cloning.** All call sites are known, so clone a
   function per call site's known-constant arguments *without* inlining it
   — the enabling effect of inlining at a fraction of the size cost.

## B. The aliasing family — LLVM's biggest structural weakness

LLVM burns enormous effort on alias analysis and still gives up constantly;
`restrict` is opt-in and unverified. wrela's `values.exclusivity.no-overlap`
makes non-aliasing *checked*.

8. **Unconditional load/store reordering and elimination.** No may-alias
   barriers on LICM, PRE, dead-store elimination, or store forwarding.
9. **Vectorization legality for free** — see §H, where this becomes the
   ffmpeg-parity argument.
10. **Store-to-load forwarding across arbitrary distances**, since nothing
    can have written the location in between.

## C. The layout-ownership family

11. **Force the alignment the codegen wants**: 16 B so stores never
    straddle (SOG §4.5), 64 B for line-aligned bulk paths, 32 B for branch
    targets (§4.8). LLVM must honor declared layouts; wrela chooses them.
12. **Hot/cold field splitting** (ladder 4a) — LLVM cannot reorder fields
    in general.
13. **Per-array layout choice.** AoS or SoA decided *per array* from
    measured access, not globally by language rule. Nobody does this.
14. **Alignment-specialized bulk paths.** No unaligned prologue or peeling
    loop, because alignment is a build-time fact.
15. **`ADR`-only addressing** with a link-time range proof (ladder 6b/6k) —
    LLVM must stay relocation-safe and cannot.

## D. The known-shape family

16. **No remainder loops — and the good trick: the compiler picks the
    size.** LLVM must emit a scalar epilogue for `n % 4`. wrela knows array
    sizes at comptime and *chooses pool sizes*, so it can round a pool up to
    a SIMD-friendly multiple and delete the remainder loop entirely. LLVM
    can never change your array's size. This is a genuine "compiler does
    what only a human could" moment.
17. **No trip-count dispatch.** LLVM emits `if (n < VF) goto scalar`;
    wrela's bounds are comptime or measured, so the branch never exists.
18. **Unroll factors from measured trips**, and — equally important — *no*
    unroll where the OoO window already handles it (ladder prior 3).

## E. The value-range family — automating "the human knows the invariants"

This is the least explored and possibly the highest-value untapped axis.
wrela has **sized integer types with checked arithmetic**: if a program is
accepted, every arithmetic result is provably in range. That is a free
range oracle that LLVM must reconstruct with `computeKnownBits` and
routinely loses across call boundaries.

19. **Type-driven width selection.** The A76 payoff is concrete and large:
    `MADD` **W-form is 2-cycle at 1/cycle throughput**, while **X-form is
    4-cycle at 1/3 throughput and stalls the M pipe 2 extra cycles**
    (SOG §3.6 notes 2/4). A type-known-≤32-bit multiply should never emit
    X-form. Same story for divides, which block the only M pipe.
20. **Range-proved check elision.** Propagate declared ranges to prove
    overflow and bounds checks dead — the *provable* subset of check
    removal, which is the only subset allowed (ladder 2d).
21. **Range-proved canonicalization elision.** `narrow_to_width`'s LSL/LSR
    pair is dead whenever the range proves the value already canonical.
22. **Range-driven representation choice.** A value proven < 2^31 can live
    in a `W` register and be stored as 4 bytes — feeding the compressed
    pointer/handle packing in ladder 6m.

## F. The whole-program-frequency family — PGO without PGO

23. **Measured block frequency is the default.** LLVM's PGO needs an
    instrumented build, a training run, and a profile that goes stale.
    wrela's Lane 2 sidecar is committed next to the source and validated by
    Lane 3 host agreement. Every frequency-driven decision below is
    therefore *ordinary*, not opt-in.
24. **Basic-block-grain hot/cold code layout.** Pack the measured hot path
    contiguously so I-fetch is near-perfect and cold paths leave the 64 KiB
    L1I entirely. This directly attacks the 93–98 KB-text-vs-64 KiB-L1I
    problem, and it is the one opt that makes *every other* opt's footprint
    cheaper.
25. **Fallthrough = measured-likely path**, on every branch, everywhere.
26. **Call-site-frequency-driven specialization** instead of heuristic
    inlining thresholds.

## G. The single-machine family

27. Everything in ladder Tier 6 — TBI tagging, `DC ZVA`, block page tables,
    bitmask-immediate SWAR, `RBIT`+`CLZ`, `CCMP` chains, `UBFX`,
    crypto-as-mixer, `PMULL` Morton spread. Unconditional because there is
    no feature detection and no other target to keep working.

## H. The vectorizer: where ffmpeg parity is actually defensible

The user's target deserves a direct answer, and this section is it.

**Why LLVM's auto-vectorizer produces bloated, cautious code** — four
reasons, all of them things it *must* do:

1. runtime alias checks guarding the vector loop;
2. a scalar fallback loop for when those checks fail;
3. a remainder/epilogue loop for `n % VF`;
4. an alignment peeling loop.

**wrela removes all four structurally** (§B, §D, §C). So a wrela vectorizer
emits: no alias checks, no scalar fallback, no remainder, no peeling —
*just the vector loop*.

That is not a marginally better vectorizer. **That is the shape hand-written
assembly has**, and it is precisely why ffmpeg's asm is smaller and faster
than compiler output for the same algorithm: it isn't carrying four
contingencies that cannot occur.

Add §E's range information (which lanes can overflow, what widths are
needed) and §F's real frequencies (which loops deserve it), and the
legality-and-profitability questions that make auto-vectorization
unreliable everywhere else are answerable here.

**So: for a given algorithm expressed in wrela, hand-asm-quality vector
output is a defensible target.** Not because the vectorizer would be
cleverer than LLVM's, but because it has less to be afraid of.

## I. Moonshot: verified superoptimization

The "god tier" answer, and it is wrela-shaped in a way it is not
LLVM-shaped.

**The idea.** For the measured-hot basic blocks only (Lane 2 says which —
there will be few), search the space of equivalent instruction sequences
and keep the cheapest under the M20 cost model. Commit the discovered
rewrites as ordinary in-code patterns, so compile time pays nothing at
build time — the search is offline, its output is a table.

**Why wrela can and LLVM effectively cannot:**
- **One target.** No need for a rewrite to be profitable or even valid
  anywhere else. LLVM's peephole patterns must generalize.
- **A perfect equivalence oracle.** `diff-eval` plus byte-identical
  deterministic replay means "is this sequence equivalent?" is *decidable
  by execution* on this machine, not merely argued. This is the piece
  every superoptimizer struggles with and wrela already built for other
  reasons.
- **A real cost model to rank candidates** — that is what M20 is.
- **A tiny, measured hot set.** Superoptimization fails on whole programs
  and succeeds on kernels; Lane 2 tells you exactly which kernels.
- **Offline is allowed.** The cleverness budget explicitly permits offline
  research that produces a committed, locked artifact.

LLVM's peephole tables were written by hand over twenty years. wrela could
**synthesize its own, verified, for its one target** — and that is a
credible path to "better than the patterns a human would have thought to
write," which is the actual definition of beating hand asm.

**Kill conditions.** If the search space for realistic block sizes is
intractable even offline, it dies. If the discovered rewrites are not
expressible as stable patterns (i.e. every one is a special case), the
table never converges and it dies. First falsifiable step: take the single
hottest block from `boot-actors`, enumerate 3-instruction equivalents under
the cost model, and see whether anything beats what codegen emits today.

## The honest boundary

Rows 9 and 10 of the frame survive, and pretending otherwise would be
dishonest:

- **Compile time is a product number here too** (sealed images ⇒ every
  update is a full rebuild; `xtask check` is the agent inner loop). Every
  item above must fit that budget, which is exactly the constraint that
  makes LLVM cut heuristic corners. The escape hatch is that expensive
  search can be *offline* with committed results (§I), not that wrela is
  exempt.
- **No compiler discovers a better algorithm.** ffmpeg's asm advantage is
  roughly half "no compiler contingencies" (wrela can match, §H) and half
  "an expert chose the DCT butterfly ordering and the shuffle sequence"
  (wrela cannot). **The honest claim is hand-asm-quality codegen for the
  algorithm you wrote** — which is the whole prize, since it means writing
  ordinary wrela gets you what writing assembly would have.

## Suggested reading order for anyone picking this up

1. §A is the highest-leverage and the most clearly LLVM-impossible; the
   interprocedural allocator is the keystone item in the whole project.
2. §F.24 (hot/cold block layout) is the cheapest large win and it makes
   every code-growing opt affordable.
3. §E.19 (W-form vs X-form multiply) is a small, immediate, verified win
   worth doing as a warm-up.
4. §H matters only once something in the stdlib has a real data-parallel
   loop — but when it does, it is the ffmpeg-parity argument.
5. §I is a research project. Do not start it before §A and §F exist,
   because it optimizes what the rest of the pipeline hands it.
