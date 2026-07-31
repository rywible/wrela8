# Pixels: the field renderer, after measurement

**Status: DESIGN (2026-07-31).** The successor to
[graphics.md](graphics.md) for the parts its §16 benchmark settled, and a
correction to it where the measurements disagreed.

graphics.md said: *"It is promoted to a normative chapter
(`docs/language/07-pixels.md`) only after §16's benchmark returns, and only
for the parts the benchmark actually settles."* The benchmark has returned.
`crates/wrela-fieldprobe` is the instrument, `fieldprobe-report.txt` its
output. §16.4's kill criterion is evaluated and **all three scenes
survive**.

This document is still not `docs/language/`. Two reasons: no measurement
here ran on a Pi 5 (§16.1's M4-proxy caveat stands in full), and the single
largest assumption underneath the headline result — that a packet-wide tape
interpreter amortises its dispatch — was never measured at all. What has
changed is that the *shape* of the design is now decided by evidence rather
than by argument, and three things graphics.md was open to are now known to
be dead ends.

Decision block **1900–1999** requested. Not claimed until a human activates.

---

## 0. What the measurements changed

Five findings reshaped the design. Two are wins, two are demolitions, one
is a correction to how everything else was being read.

| Finding | Effect |
| --- | --- |
| **Reconstruction factor grows with output resolution** (4.76→23.96, 14.0→70.1) | 4K becomes reachable; every earlier number was read off the worst data point |
| **Tape pruning delivers** — 173→28 ops, 312→**median 1** | §2.2 is confirmed as the load-bearing mechanism |
| **A baked octree is *slower*** (0.69–0.98×) | no spatial acceleration structure, ever |
| **Volumetric light bakes do not carry this geometry** (p95 0.27) | no irradiance/AO/shadow volume |
| **Interval arithmetic beats affine arithmetic on box SDFs** (20–50×) | `eval_range` must carry both domains |

The correction is the important one and it is worth stating plainly: for
most of this design's life, the reconstruction factor was measured at
512×288 and then used to reason about 4K. That silently assumes the factor
is resolution-independent. It is not, and the direction is favourable — see
§2.

---

## 1. The arithmetic, remeasured

graphics.md §1 modelled **~4,500 FLOP/pixel** for a frame under the
classical amortisations. Measured, with tape pruning, slab-advance
traversal and over-relaxation:

| scene | FLOP/pixel | note |
| --- | --- | --- |
| colonnade | **3,390** | hard-surface architecture, 2-octave ground displacement |
| colonnade-flat | **2,280** | the same scene, displacement off |
| melee | **2,806** | 4-figure `smin` cluster, k=0.08 limbs |

§1's model was accurate to about 25% on the pessimistic side. That is a
better outcome than it sounds: the model was built from first principles
before any of the mechanisms existed.

Composition (colonnade), which decides where effort is worth spending:

| term | share |
| --- | --- |
| primary visibility | 35.7% |
| shading arithmetic (§1's model, unchanged) | 16.3% |
| AO + GI taps | 15.5% |
| shadow rays | 12.9% |
| traversal — affine classify + prune | 10.8% |
| post | 8.9% |

**Displacement costs 1.49×** (3,390 vs 2,280). Two octaves at amplitude
0.02 on a large grazing surface. That is the single most expensive
art-direction decision in the frame and it is the difference between 4K60
closing comfortably and closing on a knife edge (§9). It belongs in the
§6.5 cost diagnostic:

```
warn[FIELD_DISPLACE_COST]: 2 octaves at amplitude 0.020 on 'ground' raise
  frame cost 1.49x (measured class: large-area grazing surface).
  --> world/terrain.wr:9
  help: at @output(3840, 2160, 60) this is the term that does not fit.
```

---

## 2. The output representation

**This is the central finding of the whole exercise.** The renderer's
output is not a grid of pixels; it is a set of quadratic patches bounded by
analytic discontinuity curves. The guest rasterises that representation to
pixels itself (§7), but the *representation* is what makes the arithmetic
work.

### 2.1 True discontinuity density is small, and shrinks with resolution

Measured per pixel, with no cells involved — march every pixel, ask whether
any 4-neighbour differs in hit/miss or in relative depth by >5%:

| scene | 512×288 | 1024×576 | 1920×1080 | 3840×2160 |
| --- | --- | --- | --- | --- |
| colonnade | 7.62% | 3.87% | 2.08% | **1.06%** |
| melee | 2.38% | 1.20% | 0.64% | **0.32%** |

Edges are a 1-D set. Their pixel count grows linearly with resolution while
the frame grows quadratically, so the edge *fraction* halves with every
doubling. That is not an observation about these scenes; it is a property
of piecewise-smooth images.

### 2.2 The reconstruction factor therefore grows with resolution

A patch is a quadratic fitted to *world* geometry. Raising the output
resolution does not create patches at the rate it creates pixels. Two
effects fight — patches survive resolution increases, but the fit tolerance
is stated in pixels of parallax and tightens as the footprint shrinks — and
the empirical answer is decisive:

| scene | 512×288 | 1024×576 | 1920×1080 | **3840×2160** |
| --- | --- | --- | --- | --- |
| colonnade | 4.76× | 7.98× | 13.32× | **23.96×** |
| melee | 14.02× | 24.29× | 40.41× | **70.10×** |

Patch samples grow 14.8× while pixels grow 64×. At 4K, melee needs 118,320
shaded samples for 8.29M pixels — about 8,500 patches averaging 31×31
pixels, plus 26,890 edge samples.

### 2.3 Three rules the fit must obey

Each was found by measuring a version that violated it.

1. **Fit inverse *view-axis* depth, not depth, and not inverse ray
   distance.** For a plane, `1/z` is exactly affine in screen coordinates —
   this is why rasterizers have interpolated `1/z` since 1995. Distance
   along the ray carries a `sqrt(1+u²+v²)` factor that destroys the
   affinity, and destroys it worst on the large planar surfaces where the
   patch representation has to win. Worth 40%→72% pass rate on melee's 32px
   cells.
2. **Judge the fit only on non-edge pixels.** Letting a single edge pixel
   condemn its cell is what held reconstruction at 1.05× instead of 23.96×.
   Axis-aligned cells cannot align to a diagonal edge; a curve-bounded
   region splits rather than fails.
3. **Subdivide adaptively.** A uniform grid measures the wrong thing — the
   question is not "what cell size passes everywhere" but "how many samples
   does the whole frame need".

### 2.4 What this buys, and what it costs

It multiplies the *entire* frame, not a term of it — which is why it
outranks every optimisation in §3 and every bake in §4.

It costs shading bandwidth. At 70× reconstruction the guest shades one
sample per 70 output pixels, so **material detail is bandlimited to sample
density**. §9.3's band 3 (micro normal and roughness perturbation) cannot
be resolved at output resolution without per-pixel evaluation, which does
not fit at 4K (§9). For §9.5's porcelain/clay direction that is a match.
For fine surface texture it is not, and that is the real quality trade at
4K — not a throughput one.

---

## 3. The visibility engine, as measured

graphics.md §2's five mechanisms, scored.

### 3.1 Tape pruning is the load-bearing mechanism (§2.2 — confirmed)

Live ops by subdivision depth, recursing on the pruned tape:

| scene | full tape | depth 3 (8px) | depth 4 (4px), mean / median |
| --- | --- | --- | --- |
| colonnade | 173 ops | 33.4 | 28.3 / **28** |
| melee | 312 ops | 29.8 | 22.5 / **1** |

§16.4's kill threshold was ">100 ops at depth 3". Both are far under, and
melee — the scene graphics.md called *"the scene that will tell the
truth"* — prunes better than the architecture scene, because half its frame
is one ground plane that collapses to a single op.

**A pruned tape is valid only over the region it was pruned for.** This is
a correctness invariant, not a detail: recursing into a subregion while
sweeping past that region proved 37.8% of a frame empty while the marcher
hit 15,012 of those pixels. The renderer prunes twice per cell — once over
`[t_a, t_far]` for the children to recurse on, once over the found slab for
the work executed there.

### 3.2 `eval_range` must carry two domains (§2.1 — corrected)

graphics.md §2.1 asserts *"plain interval arithmetic is too loose to use"*.
On distance fields that is **backwards**. Measured on the same wedges:

| | proven empty by AA | by plain IA | mean width ratio |
| --- | --- | --- | --- |
| colonnade | 27.2% | **78.2%** | IA 25× tighter |
| melee | 10.5% | **79.7%** | IA 50× tighter |

The cause is structural: `min(x, 0)` — the *inside* term of every box SDF —
is exact under intervals, because anything positive collapses to zero.
Under affine arithmetic it routes through `abs`, whose Chebyshev line
leaves a symmetric error band that cannot collapse.

Affine arithmetic still earns its place: it is tighter wherever
correlations cancel, which is most of a CSG tree. **Both domains propagate
independently and decisions read the intersection.** Writing the
intersection back into the affine form collapses it at the first slot IA
wins and switches the affine domain off entirely — measured as an AA/IA
width ratio of exactly 1.00× at every slot, which is not agreement.

### 3.3 Continuation and over-relaxation (§2.5 — confirmed, modestly)

- **Continuation on the hit manifold**: 1.79–2.08× fewer eval-equivalents
  than marching, 91–99.8% converging to the true hit, worst depth error
  0.10 px of parallax. `∂t/∂x = −F_x/F_t` with both terms from one
  `eval_grad`.
- **Over-relaxation** (Keinert 2014): **1.09×**, well under the paper's
  claimed 30–50%. Its overshoot backtrack must be computed *before* the
  relaxation factor is reset, or it silently tunnels.

### 3.4 "Solve, do not march" is scene-dependent (§2.3 — measured)

Blend-band ray fraction — the share of traversed ray length where `smin`
deviates from `min`, and closed-form solving is therefore unavailable:

| colonnade | melee |
| --- | --- |
| **17.9%** | **96.3%** |

graphics.md guessed 5% and 40%. §2.3 is a real lever on hard-surface
architecture and **nearly inapplicable to character work**. Its actual
value on the 82% of colonnade ray length that *is* solvable remains
unmeasured.

### 3.5 The interior certificate is weak, and it does not matter

Certified-interior screen area came in at 2.2%/12.5%, far under §2.1's
expectation that *"the majority of screen area is interior"*. The cause is
that `min(m, 0)` and `max(q, 0)` in a box SDF each have an ambiguous
derivative at the face even though that non-smoothness cancels in their
sum; fixing it needs fused box primitives with analytic gradients.

**A perfect certificate is worth only 1.03–1.11× of the frame.** It is not
worth building. Recorded so nobody spends a milestone on it.

---

## 4. What was tried and does not work

Both were built, measured, and are recorded here so they are not
re-proposed.

### 4.1 Spatial acceleration structures — **rejected, measured**

A baked octree over the static field: pruning precomputed per cell,
empty/full certified by affine arithmetic, degree-2 proxies seeding
closed-form roots verified against the true tape.

| | atlas | live sphere tracing | ratio |
| --- | --- | --- | --- |
| melee | 1,934 FLOP/px | 1,336 | **0.69×** |
| colonnade | 1,618 | 1,577 | **0.97×** |

Slower, at every depth and every epsilon, on both scenes. Adding proxies
made it *worse*.

The reason is structural rather than a tuning failure: **an SDF already is
an acceleration structure**, and its empty-space skipping is *exact* where
an octree's is conservative. The tree pays 30.5 node visits per pixel to
replace a mechanism that was free, and only 18% of boundary cells yielded a
usable closed-form solve. Bakes are transformative for renderers with no
distance oracle. This one has one.

### 4.2 Volumetric light bakes — **rejected, measured**

| bake | cell | mean err | p95 | max |
| --- | --- | --- | --- | --- |
| ambient occlusion | 0.125 | 0.027 | **0.169** | 0.391 |
| sun visibility | 0.125 | 0.047 | **0.267** | 0.783 |

A p95 of 0.17–0.27 on a 0–1 scale is visible banding. Memory was never the
constraint (0.03 MB as `u8`).

Two distinct failures. **AO is the wrong target**: §8 already prices it at
*"4–5 distance samples along the normal, near-free"*, so the bake replaces
something cheap with something inaccurate. **Sun visibility is the right
target** — 5 evals per hit, long rays, 12.9% of the frame — and it fails
differently: its error *flattens* with resolution instead of halving,
because penumbra boundaries are discontinuities no grid resolution
resolves.

The normal-offset fix for volumetric AO made it worse (0.060 → 0.218): it
biases the lookup toward free-space occlusion while the truth is surface
occlusion.

### 4.3 The shape of both failures

They fail the same way, and the lesson generalises: **stop trying to
accelerate what the field is already good at.** Every real win measured in
this document came from the output representation (§2), not from the
tracer. That is the design's centre of gravity and it should stay there.

---

## 5. Lighting

§8's stack stands, unamended, because §4.2 removed the alternative. AO from
distance taps, one sphere-traced shadow ray with the `min(k·d/t)` penumbra
estimator, GI amortised through the world-space probe clipmap.

The one change: **probes are the only sanctioned cross-frame cache**, and
§4.3's invalidation rule (invalidate probes whose radius intersects a moved
instance's swept bounds) is now the *whole* caching story rather than one
member of a family. There is no light volume, no irradiance grid, no
brick map.

---

## 6. Motion

§16.2's camera whip, costed **every frame** rather than at one
representative pose, because a frame budget is set by the worst frame.
Sixteen frames ramping to 15.75°/frame:

| frame | deg/frame | primary FLOP/px | reproj hint | verified |
| --- | --- | --- | --- | --- |
| 0 | 0.00 | 1920 | — | — |
| 4 | 8.80 | 1937 | 41.2% | 71.3% |
| **7** | **15.75** | **2331** | **34.2%** | 68.5% |
| 11 | 8.80 | 1713 | 40.7% | 69.0% |
| 15 | 0.00 | 1688 | 53.5% | 100% |

**Peak/mean 1.21×, and the peak lags peak angular rate by one frame.**

The important part is that the two curves move together: cost rises 1.21×
while reprojection coverage falls 52% → 33%. §4's correctness claim
survives motion (hints that exist verify at 98.6%); its *coverage* does
not. The frames that most need reprojection get the least of it.

This is what §4.4's velocity-scheduled resolution is for, and the table
above is the curve to schedule against. It is also why reconstruction must
happen guest-side (§7): a disoccluded region can be re-marched only by
whoever holds the field.

---

## 7. Everything happens in the guest

**Decided.** The guest shades, reconstructs, rasterises and writes pixels.
The VMM scans out and does nothing else.

This was decided on product grounds — every technique belongs to wrela —
and the arithmetic does not object. An earlier draft had the guest emit a
patch/curve record stream for a host-side decoder. That was wrong on
bandwidth:

| | guest writes pixels | guest emits records |
| --- | --- | --- |
| guest → DRAM | 1.99 GB/s | 0.28–0.83 |
| host reads records | — | 0.28–0.83 |
| host → DRAM | — | 1.99 |
| display reads | 1.99 | 1.99 |
| **total** | **~4.0 GB/s** | **~5.7 GB/s** |

The framebuffer write and the scanout read happen either way; records are
an *additional* round trip. §5's cache rule is satisfied by reconstructing
**tiled** — build a 64×64 output tile in L1, write it, move on — which is
§5's tile fusion extended one stage.

The cost is ~11.5 GFLOP/s of idle host core, about 40% of system compute.
Concretely that is the margin at 4K60 (§9). It is accepted.

**Frame-rate decoupling is better guest-side anyway.** Shading at 30 Hz and
rasterising at 60 Hz works because the two costs are wildly asymmetric —
2,506 FLOP per sample against ~16 ops per pixel. Running the raster twice
is cheap; running the shade twice is not. And the guest can re-march
disocclusions, which a decoder never could.

**Bandwidth, separately.** The two-plane composite — a 1080p interior plane
plus a sparse native-resolution edge plane, composited by the HVS — cuts
scanout traffic from ~4.0 GB/s to ~1.05 GB/s, because edge pixels are
0.32–1.06% of a 4K frame. That is orthogonal to who computes the pixels and
is the real bandwidth lever. It depends on §14.2's open HVS questions.

---

## 8. The compiler surface

### 8.1 There is no new IR

The patch/curve representation is a **struct in a pool**, not an IR. It
earned an IR-shaped name only while it crossed the guest/host boundary,
where a schema, an encoder and a decoder would have needed to be kept in
agreement. Guest-side it is a variable.

**FieldWir** may not need to be a separate IR either. It is the comptime
evaluator with one additional value kind — `Symbolic(node)` — so that
evaluating `p.x * 2.0` with `p` symbolic yields a node instead of a number.
That is in the spirit of *evaluator before backend*: the evaluator is the
reference implementation of the semantics, and a symbolic trace of itself
is an extension rather than a parallel toolchain.

### 8.2 The derived programs are stdlib interpreters, not compiler passes

`eval_range`, `eval_grad` and `eval_dt` are interpreters over the tape,
written in wrela, and they sit inside §7's boundary clause without widening
it — affine arithmetic and forward-mode duals *are* field evaluation, in a
different arithmetic domain.

They **have** to be interpreters: pruning depends on the tile, so the
pruned tape is runtime data and cannot be specialised at comptime. §6.3's
line holds with a clarification — topology is comptime, parameters are
runtime, and *pruning is per-region and therefore runtime*.

### 8.3 What the compiler actually gains

| what | where |
| --- | --- |
| parse `@field` | existing parser |
| check the subset (total, pure, bounded loops) | one whole-tree sema pass |
| abstract-interpret with `p` symbolic → DAG | evaluator + one value kind |
| flatten DAG → tape, with CSE | one small pass |
| emit the tape as **data** | existing data sections |
| cost model over the tape | extends `CostRule` |

One sema pass, one evaluator extension, one flatten pass. Everything else —
classify, prune, fit, trace, raster, shade — is ordinary wrela through the
existing pipeline.

The revised §6.2 table: keep `eval`, `eval_range` (dual-domain), `eval_grad`,
`eval_dt` (fused into the range pass), `tape`, `cost`. **Cut `eval_hess`**
— never needed; the patch fit is empirical and the Hessian bound was never
the binding constraint.

---

## 9. Execution: the packet interpreter

### 9.1 CSG is branch-free, which is what makes this work

Union, intersection and subtraction are `min`/`max`, which are single NEON
instructions. Every lane of a packet takes the same path through the tape —
there is no divergence, ever. That is what permits interpreting op-by-op
across a packet and amortising dispatch, and it is the assumption the
entire cost model rests on.

### 9.2 Two packet axes

- **The marcher packetises over rays** — N rays through one pruned tape, SoA.
- **The classifier packetises over tiles** — affine arithmetic is
  per-region, so sibling tiles evaluate together.

SoA is not a preference, it is the instruction-selection decision: in SoA a
dot product is 3 `FMLA`s with zero shuffles; in AoS it needs horizontal
reductions. Every dot in the renderer — `∇f·d̂`, `n·l`, `v·v` inside
`length` — is on that path.

### 9.3 Packet width is decided by register pressure, and fp16 wins twice

32 NEON registers, 128 bits each, against the measured median pruned tape:

| packet | regs/slot | 28-op tape |
| --- | --- | --- |
| 4× fp32 | 1 | **28 — fits** |
| 8× fp16 | 1 | **28 — fits** |
| 8× fp32 | 2 | 56 — spills |
| 16× fp32 | 4 | 112 — spills |

**fp16 buys 2× throughput and 2× packet width at identical register
pressure** — §3's two wins compound rather than trade.

This exposes the interpreter's central tension, now quantified:
register-resident (slots never touch memory, but dispatch amortises only
4–8×) against streaming (dispatch amortises arbitrarily, but every slot
round-trips the frame at ~5 cycles, and store-data uops share the V pipes).
A hybrid keyed on tape length is probably right; the measured distribution
is 28 median / 82 max (colonnade) and 1 median / 179 max (melee).

### 9.4 This activates Tiers 9–10

[opts-ladder.md](opts-ladder.md) lists *"SIMD + vectorizer (Tiers 9–10) —
gated on the pixels rung; when that activates, this becomes urgent rather
than optional."* This document is that rung.

**9d is answered: auto-vectorisation suffices; the language does not
change.** The interpreter's inner loop per op has a comptime-constant trip
count, no aliasing, no control flow and a uniform operation — Tier 10's
"legality is a type-system consequence" argument made concrete. The output
is just the vector loop, with no alias-check prologue or scalar epilogue.
This agrees with the ladder's own rejection of explicit vector types as a
language surface.

**One decision is unrecoverable if deferred.** `FRSQRTE` + Newton is the
right lowering for `length` and `normalize`, but freeze 1407 requires `dev`
and `release` to agree bit-for-bit. So the **stdlib must define `rsqrt` as
the explicit Newton sequence**, so both modes compute the identical thing
and `release` only selects better instructions for the same arithmetic. A
`release`-only strength reduction of `1/sqrt(x)` breaks `diff-eval`.

### 9.5 The cost model is conservative

Every FLOP figure in this document charges raw op counts — roughly `dev`
semantics. `release` already measures −15.5% on the product tier before
Tiers 9–10 exist, and the packet interpreter is the best vectorisation
target in the codebase. Two things are missing in the other direction: the
op weights are estimates rather than SOG rows (M20 inventory row 35's
trigger has fired; they belong in `bench/a76-pi5.toml`), and store-data /
V-pipe contention is not modelled at all.

---

## 10. The resolution ladder

Budget: 2.4 render-core-equivalents × 16 FLOP/cycle × 2.4 GHz = 92 GFLOP/s
peak, **27.6 GFLOP/s at §1's 30%-sustained assumption**. Raster is ~16 ops
per output pixel. `C` is per-sample cost; `R` is §2.2's measured
reconstruction factor at that resolution.

| mode | melee | colonnade-flat | colonnade |
| --- | --- | --- | --- |
| **1080p60**, shade at 60 | 35% | 74% | 67% \* |
| **4K30**, shade at 30 | 47% | 61% \* | 81% \* |
| **4K60**, shade at 60 | 93% | 178% ✗ | 162% ✗ |
| **4K60**, shade at 30 + raster at 60 | 61% | 72% \* | **96%** \* |

\* with the measured optimisation stack: fp16 on the 44.6% that is shadow +
AO/GI + shading (§3), continuation at its measured 2.01× on the 92.4% of
primary that is smooth.

**1080p60 fits everywhere with room. 4K60 fits everywhere if shading runs
at 30 Hz and the representation is re-rastered at 60.** colonnade sits at
96% of budget, which means no headroom for a whip — §6 measured peak/mean
at 1.21× and 61% disocclusion, so colonnade at 4K60 depends on §4.4's
velocity schedule rather than on slack. Dropping the ground displacement
(§1) or reaching 35% sustained removes that dependency.

---

## 11. Rules the measurements forced

Each was found by measuring an implementation that violated it. They are
stdlib implementation rules, locked by `diff-eval` and unit tests.

1. **`smin` returns its operand verbatim when saturated.** `b + (a−b)` is
   not bit-identical to `a` in floating point. Written naively, §2.3's
   "deviates only inside the band" and §7's bit-identity gate contradict
   each other by one ulp on every pruned blend.
2. **`smin`'s enclosure uses its algebra, not its formula**:
   `min(a,b) − k/4 ≤ smin(a,b,k) ≤ min(a,b)`, both ends attained.
   Evaluating the expression in the interval domain enclosed a sky tile 15
   units above the geometry as `[−29.3, +8.0]` and reported 0.00% exterior
   on a 45%-sky scene.
3. **`eval_range` carries an affine form and an interval, propagated
   independently, intersected for decisions** (§3.2).
4. **A pruned tape is valid only over its own region** (§3.1).
5. **`length` is one primitive with a Cauchy–Schwarz derivative bound**
   (`|d|v|/dt| ≤ |v'|`). Lowered to `sqrt(Σx²)`, the chain rule divides by
   `|v|`, which is identically zero inside every box.
6. **An affine form cannot be intersected with a box.** It asserts
   `truth(ε) ∈ [c + L(ε) ± e]` for every ε; neither shifting `c` nor
   shrinking `e` is a valid tightening. Keep the form, or collapse to an
   opaque interval.
7. **`rsqrt` is defined as an explicit Newton sequence in stdlib** (§9.4).

---

## 12. Oracles

Extends graphics.md §15.

| what changes | its oracle |
| --- | --- |
| a FieldWir pass | that pass's dump golden |
| `eval_range` | the AA-vs-IA width ratio, pinned — two bugs hid there |
| the tape interpreter | `diff-eval`, **bit-identical** against compiled `eval` |
| the patch fitter | reconstruction factor by resolution, pinned |
| the classifier | **an independent march must never hit a pixel proved empty** |
| a frame | framebuffer goldens on named shots |
| renderer performance | a `bench` lane + a re-locked threshold |

The gate in bold is the one that matters most and is the least obvious. A
pixel with no leaf was *proved* to contain no surface; if the marcher finds
one there, every area fraction in the report is void. It caught a bug that
had silently held for four experiments.

### 12.1 The instrument, and a warning

`crates/wrela-fieldprobe` is ~3,000 lines, zero dependencies, and depends
on nothing in the compiler — deliberately, since §16's whole point is
producing these numbers *before* FieldWir exists.

**It rejected itself nine times before any number was quoted.** Every one
of those failures produced a plausible result rather than a crash:
enclosures that were too *tight* (reporting geometry as absent), an
enclosure that diverged to 5×10¹⁷ (which no test fails, because an infinite
bound excludes nothing), a proxy certificate whose Lipschitz argument
needed 80 samples per axis per cell and certified zero of 66,538, and a
recursion that proved 37.8% of a frame empty while the marcher hit 10% of
it.

Three hypotheses about the reconstruction limit were tested and **refuted**
— fit space twice, marcher convergence once — before the real cause (cell
alignment) was found. One control scene exists solely because a rejection
rate had two candidate explanations and the obvious one turned out to be
wrong.

The numbers in this document are worth something only because of those
gates. Any successor implementation should carry the same ones.

---

## 13. Still unmeasured

In descending order of how much rests on them.

1. **Packet-interpreter dispatch amortisation.** The largest assumption
   under §10. Every FLOP figure assumes op-by-op interpretation amortises
   across a packet; §16.1 says the M4 proxy biases interpreter overhead
   optimistic. Needs an A76.
2. **§1's 30%-sustained figure.** Every projection scales linearly in it,
   and the proxy structurally cannot measure it. colonnade's 4K60 verdict
   flips on it alone.
3. **Deforming fields.** *Both scenes are static.* §4.2's time-Lipschitz
   certificates, §10.1's implicit skinning, and the cost of re-pruning a
   moving `smin` cluster every frame are entirely untested. §6 moved the
   camera, not the geometry. **This is the next benchmark, not the next
   feature.**
4. **§14.2's display path.** Whether the HVS scales during scanout,
   composites two planes at different scales, and its maximum upscale
   factor. A datasheet question that decides §7's bandwidth lever.
5. **The edge-sample charge.** §2 charges one full shaded sample per edge
   pixel. A curve representation should beat that — an edge needs coverage
   plus its two neighbouring patches, not an independent shade —
   conservatively by 2–3×, which is exactly colonnade's remaining margin.
6. **§2.3's closed-form solve.** 82% of colonnade ray length is outside
   blend bands; what that buys is unmeasured.
7. **Store-data / V-pipe contention**, and op weights sourced from the SOG
   rather than estimated (§9.5).

---

## 14. Decided

1. **The output representation is patches bounded by discontinuity curves**,
   fitted in inverse view-axis depth, judged on non-edge pixels, subdivided
   adaptively (§2).
2. **Everything happens in the guest.** The VMM scans out and nothing more
   (§7).
3. **No spatial acceleration structure** — measured slower (§4.1).
4. **No volumetric light bake** — measured inaccurate (§4.2).
5. **`eval_range` is dual-domain** (§3.2).
6. **No new IR.** The frame representation is a struct; FieldWir is the
   evaluator plus a symbolic value; the derived programs are stdlib
   interpreters (§8).
7. **The interior certificate is not worth strengthening** — 1.03–1.11×
   (§3.5).
8. **Auto-vectorisation, not a vector language surface** (§9.4).
9. **`rsqrt` is an explicit Newton sequence in stdlib** (§9.4).
10. **4K60 is the target, at 30 Hz shading and 60 Hz raster**; 1080p60 is
    the floor (§10).

## 15. Order of work

1. **§14.1's bit-exact math library.** Unchanged, and the measurements
   reinforce it: every gate that caught a bug here was a bit-identity or
   containment check. Without `sin`/`sqrt`/FMA agreeing bit-for-bit between
   the comptime evaluator and codegen, `diff-eval` is not a gate and §7's
   tape-vs-compiled clause is a smoke test. `rsqrt`'s definition (§9.4)
   lands here.
2. **The seven rules of §11**, written into the spec before code is written
   against them.
3. **Pin `fieldprobe`'s counts as a golden**, and add it to
   `cargo xtask check`. §16.1 asked for this so a later Pi 5 run is a
   one-variable experiment; the numbers currently live in a text file
   nothing enforces.
4. **The deformation benchmark** (§13.3). Before FieldWir, not after — it
   is the one remaining unknown that could change the design's shape rather
   than its constants.
5. **FieldWir**: the sema pass, the evaluator extension, the flatten pass,
   and their dump goldens.
6. **The stdlib renderer**: classify, prune, fit, trace, raster.
7. **Tiers 9–10**, against `codegen-pareto`'s existing land gate.
8. **§14.2's display contract.**
