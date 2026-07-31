# Graphics: the field renderer

**Status: DESIGN (2026-07-30).** Not a plan, not normative. A design
argument and an inventory, in the genre of
[beating-llvm.md](beating-llvm.md). Nothing here activates without its own
milestone plan and its own freezes.

**This document is not `docs/language/`.** Roughly a third of it rests on
numbers nobody has measured on this hardware. It is promoted to a normative
chapter (`docs/language/07-pixels.md`) only after §16's benchmark returns,
and only for the parts the benchmark actually settles. Until then, code
does **not** get to cite this file as ground truth — §1 is ground truth
because it is arithmetic; the rest is a hypothesis with an oracle attached.

Decision block **1900–1999** requested (1600–1699 M20, 1700–1799
codegen-pareto, 1800–1899 M21). Not claimed until a human activates.

## 0. The frame

The flagship is a fixed-function games console ([01 §1](../docs/language/01-model.md)).
This document asks how it renders, under five constraints taken as given:

1. Ray marching only. Fields, no meshes, no rasterizer.
2. Pi 5 CPUs only. No GPU, no VideoCore compute.
3. No external assets.
4. AAA-quality visuals.
5. Games that are fun.

The thesis in one line: **everyone else's renderer is a database of
foreign assets; this one is a single analyzable expression.** Every
mechanism below is a cash-out of that difference. Where the design is
uncomfortable, it is because the arithmetic in §1 is uncomfortable, and
§1 is not negotiable.

---

## 1. The arithmetic

This section is the constraint every other section answers to.

**The machine.** The Pi 5 has 4× Cortex-A76 @ 2.4 GHz, but **one core is
pinned to the host Linux** — the image's sealed core count is **3**, and
that is a machine-contract fact ([06](../docs/language/06-machine.md)), not
a tuning choice. Two 128-bit NEON pipes per core, FMLA `.4s` = 8
FLOP/instruction, 2/cycle → **16 fp32 FLOP/cycle/core**. Guest peak
115 GFLOP/s. Sustained on hand-tuned packet code: assume **~30% of peak**
until measured. Memory: LPDDR4X, **~8–9 GB/s effective**. Cache: 64 KiB L1I
/ 64 KiB L1D per core, 512 KiB L2 per core, **2 MiB L3 shared with the host
core**.

The VMM still contributes over a Linux-userspace equivalent — pinned cores,
no scheduler, no page faults, deterministic frame timing — but the host
core is a **live tenant in L3 and on the memory bus**, so the guest does not
get the machine to itself. Assume neither a quiet L3 nor the full 8–9 GB/s;
this makes §5 load-bearing rather than prudent.

**The game's share.** On this machine it is not core reservation — each
core runs one cooperative event loop ([01 §2](../docs/language/01-model.md)),
so a core hosts both game and render work and the split is a **time budget
within the frame**, not a core count. Reserve ~20% of frame time for sim,
physics, procedural audio (asset-free audio is synthesis, and synthesis is
not free), and the field-collision queries gameplay makes. That leaves
**~2.4 core-equivalents of render**.

**The budget.**

| | |
| --- | --- |
| Guest cores | **3** (one pinned to host Linux) |
| Render share | ~2.4 core-equivalents = 92 GFLOP/s peak |
| Sustained assumption | ~28 GFLOP/s (30% of peak) |
| Per frame @ 30 Hz | **0.92 GFLOP** |
| Per frame @ 60 Hz | **0.46 GFLOP** |

**The cost of a frame, naively.** A `map()` evaluation over a real scene,
after culling to ~5 active primitives with smooth blends, is ~150 FLOP.
Naive sphere tracing is 40–60 steps. **One primary ray is ~7,500 FLOP.** At
512×288 that is **1.1 GFLOP for primary visibility alone, against a total
frame budget of 0.92** — over before a single light. A naive full frame
(shadow and GI rays at comparable cost) is ~3× over. No amount of
resolution reduction rescues this; it has to be attacked structurally.

**The pre-pruning landing zone.** Assume the classical amortizations only
(hierarchical frustum marching, temporal reuse, fp16 secondaries): ~15
`map()` evals/pixel primary, ~5 shadow, <1 GI (probe-amortized), ~800 FLOP
shading, ~300 FLOP post ≈ **4,500 FLOP/pixel**.

    0.92 GFLOP / 4,500 = ~204,000 pixels

| Resolution | Pixels | Margin |
| --- | --- | --- |
| 480×270 | 129,600 | 36% — safe if sustained FLOPs disappoint |
| **512×288** | **147,456** | **28% — the floor** |
| 576×324 | 186,624 | 9% — the optimistic read of the same assumptions |
| 640×360 | 230,400 | **13% over** |

→ **512×288, 30 Hz shade, 60 Hz present, 1280×720 scanout.**

That is the honest floor: what the design achieves with only techniques
that are certain to work. Every mechanism in §2 exists to buy resolution
and framerate back from there.

**The third core changes the risk profile, not the design.** At four
available cores the §2 mechanisms were upside; at 2.4 render-core-
equivalents they are **load-bearing**. 512×288 upscaled 2.67× to 720p is a
long way from AAA on its own — it is carried by analytic silhouette AA
(§9.4) and a field-guided upsample, and it is not where this should land.
A disappointing benchmark now means shipping at 480×270, not 640×360.

**On the M4 proxy (§16.1), the benchmark cannot set this number** — it can
only confirm or refute the mechanisms that would move it. The resolution
target stays at the modeled floor above until it is measured on hardware.

**Bandwidth is a separate wall.** At 8–9 GB/s, a full-resolution G-buffer
round trip to DRAM is unaffordable. See §5.

---

## 2. The visibility engine

Five mechanisms, composed. The first three are the architecture; the last
two make the residue cheap.

### 2.1 Affine-arithmetic tile classification

Interval arithmetic evaluates a field over a *region* and returns
guaranteed bounds. If `0 ∉ [lo, hi]` over a segment, the surface cannot be
there — skip it without marching. This works on fields with **no Lipschitz
bound at all**, which is what kills classical sphere tracing under noise
displacement and non-isometric deformation.

Plain interval arithmetic is too loose to use (`x·x − x` over `[0,1]` gives
`[−1,1]`). **Affine arithmetic** (Comba & Stolfi; revised AA per
Fryazinov/Pasko/Comninos for implicit ray tracing) tracks correlations
between terms and is dramatically tighter.

The mechanical fact that makes this ours: **AA is a syntactic
transformation of an expression tree.** Every operator has a known AA
counterpart. The comptime pass emits `eval_range` from the same source that
emits `eval`, with no author effort (§6).

Consequence: a screen tile can be classified **entirely interior /
entirely exterior / boundary** without tracing a ray. Only boundary tiles
subdivide. On smooth geometry — most of a scene — the majority of screen
area is interior, resolved from tile-corner depths plus a Newton polish.
The renderer does not trace 250,000 rays; it adaptively subdivides the
screen until the field's own bounds say it is done.

### 2.2 Tape pruning

Keeter, *Massively Parallel Rendering of Complex Closed-Form Implicit
Surfaces* (SIGGRAPH 2020). Evaluate the field's instruction tape with
affine bounds over a tile; any `min`/`max` where one branch provably wins
has the loser **deleted from the tape**, along with everything feeding it;
recurse into subtiles with the shortened tape.

**The cost of `map()` falls with subdivision depth.** Reported behavior: a
10,000-op world becomes tens of ops two levels down. This is the one
mechanism in the design with a shape other than a constant factor, and it
rides on subdivision the renderer is doing anyway (2.1).

It also subsumes two things that would otherwise be separate systems — see
§9.2 (LOD) and §7 (runtime topology).

### 2.3 Solve, do not march

`d(p)` restricted to a ray is a 1D function `f(t)`, and the compiler can
compose it symbolically. Spheres, boxes, quadrics, tori: polynomial in `t`,
exact roots (Bernstein-basis isolation is robust and vectorizes). CSG of
solved subtrees is interval Boolean algebra along the ray — classic CSG ray
tracing, zero steps.

**`smin` deviates from plain `min` only inside a blend band of width `k`.**
Therefore marching is confined to blend bands and nonlinear warps, and
everything else is root-finding. For hard-surface environments with
smoothed seams — most game architecture — this is potentially larger than
2.2.

It is also the least certain claim in the document, because it is
parameterized by a number nobody has measured: **what fraction of ray
length lies inside a blend band.** See §16.

### 2.4 Segment tracing

Galin et al., *Segment Tracing Using Local Lipschitz Bounds* (Eurographics
2020). Global Lipschitz sphere tracing is maximally pessimistic; a **local
directional** bound over the current segment permits far larger steps.
Another FieldWir pass — each node reports its directional bound over an
interval.

Reported speedups are in the low single digits to ~10× on the paper's
scenes. **Re-read the paper before locking a bench threshold on this
number.** A threshold set from a remembered figure is a decoration, not a
lock.

Related frontier worth tracking, not building yet: Gillespie et al., *Ray
Tracing Harmonic Functions* (SIGGRAPH 2024) marches harmonic functions
using the Harnack inequality rather than a Lipschitz bound. The meta-point
is the one that matters — **"what can be sphere-traced" is not settled
math, and owning the compiler makes each new inequality a lowering rule
rather than an engine rewrite.**

### 2.5 Newton refinement and closed forms

Sphere tracing's worst behavior is the linear asymptotic crawl to the
surface and grazing rays. With the exact symbolic gradient (§6),
`t += d / -dot(∇f, dir)` converges **quadratically** once bracketed: ten
refinement steps become two or three. When pruning has reduced the local
active set to a single analytic primitive, solve in closed form and skip
refinement entirely.

Also take over-relaxation (Keinert et al. 2014): step by `w·d`, `w ∈ (1,2)`,
back off on overshoot. Thirty lines, 30–50% fewer steps, no risk.

---

## 3. Precision tiers

A76 implements Armv8.2 FP16 arithmetic: 8 lanes × 2 FLOP × 2 pipes =
**32 FLOP/cycle**, double fp32. This is a first-class tier in the field IR,
not a late optimization.

**The rule, which is not optional:** fp16 has a 10-bit mantissa.

| Quantity | Precision | Why |
| --- | --- | --- |
| Ray origin, direction, position `p` | **fp32** | world coordinates |
| Accumulated `t` | **fp32** | accumulates over 40+ steps |
| Distance *values*, per-step | fp16 where flagged | bounded magnitude, single-use |
| Shadow / GI / distant-LOD marching | fp16 | error budget is generous |
| Material and shading arithmetic | fp16 | output is 8-bit |
| Primary near-surface refinement | fp32 | sub-pixel accuracy required |

Marching *state* stays fp32; field *arithmetic* goes half where flagged.
The annotation lives per derived program (§6.2).

---

## 4. Temporal reuse

Reproject last frame's **hit distance** `t`, not its colour, and start this
frame's march from `t_prev − slack`. Because the field gives a guaranteed
lower bound on distance-to-surface, a wrong hint costs performance, never
correctness — the march *verifies* it. No ghosting, no history rejection,
no disocclusion heuristics.

**That guarantee holds only for static geometry.** If a surface moved
toward the camera by more than `slack`, the march starts past it and
tunnels. A fixed slack is a guess. Two mechanisms, in order of preference:

**4.1 Instance-velocity bound (the default).** For *rigid* instances —
most dynamic geometry — bound the slack by the maximum toward-camera
velocity of any instance whose bounds intersect this tile's frustum. Both
frames' transforms and the instance BVH already exist; this is a per-tile
scalar, `O(instances near tile)`, conservative and **exact** for rigid
motion. No per-node machinery.

**4.2 Time-Lipschitz certificate (the escape hatch).** For genuinely
*deforming* fields — a morphing blob, in-place animated displacement —
bound `|∂f/∂t|` per subtree (the compiler can: animation inputs are
expressions with declared ranges, §6.3) and use

    d(p, t+Δ) ≥ d(p, t) − L_t·Δ

so last frame's emptiness certificates **decay at a known rate** rather
than expiring. Correct but heavier, and `L_t` is poisoned globally by one
fast node unless combined with spatial locality — hence 4.1 first.

Prior art exists (the Kalra–Barr LG-surface line; 4D sphere tracing for
motion blur). Do not write this up as novel.

**4.3 Probes are a cache and must be invalidated.** The GI probe grid (§8)
is the one stale-able cache the design keeps after arguing caches out.
State the rule rather than leave it implicit: **invalidate probes whose
radius intersects a moved instance's swept bounds.** Swept bounds come free
from §6.3.

**4.4 Rate is scheduled, not heuristic.** Camera velocity is known
exactly, so the resolution/rate trade can be scheduled: fast motion → 60 Hz
at half resolution with strong motion blur (high frequencies are being
destroyed anyway); slow motion → 30 Hz at full resolution. Roughly constant
perceptual budget. Separately, reprojection failure is mostly disocclusion,
which a field detects **exactly** (the verification march fails) — so
re-march only those pixels and degradation is proportional to disocclusion
area, not uniform.

This is the resolution of a real tension: §10 promises fighting-game-grade
melee, and fast melee with a whipping camera is precisely where
reprojection is weakest. It is an unbuilt mechanism, not an open question,
but it must be measured on the camera-whip scene (§16).

---

## 5. Bandwidth and tile fusion

At ~8–9 GB/s **shared with the host Linux core**, and a 2 MiB L3 that host
core is also using, **no full-frame intermediate may be written and read
back.** This is an architectural rule of the `frame` layer, not an
optimization, and the pinned host core is what promotes it from prudent to
mandatory.

- March → normal → material → shadow → shade for a 32×32 tile happens with
  the tile's working set resident in L1/L2. Only final RGB leaves.
- **The guest hands off at internal resolution, never at scanout
  resolution.** 512×288×4B = 590 KB/frame = 35 MB/s at 60 Hz. Upscaling in
  the guest instead would be 3.7 MB/frame = 221 MB/s — and, worse, streams
  3.7 MB per frame through a 2 MiB L3 the host core shares, evicting
  everything the next tile needs. Whoever scales, scales on the host side
  (§14.2).
- A tile's **pruned tape must live in L1** — a few KB. Tape pruning
  generates data; if every tile writes a tape to DRAM it has traded FLOPs
  for bandwidth that does not exist. Size the tape arena to L1 and make
  overflow a diagnostic.
- Text footprint matters: L1I is 64 KiB and the codegen-pareto plan already
  records text at 93–98 KB. The marcher's hot loop competes for it. A tape
  *interpreter* (§7) is text-small by construction; a per-field specialized
  kernel is not. This is an argument for §7 independent of pruning.

---

## 6. FieldWir: the authoring surface

**Do not build a shader sublanguage.** The authoring language is wrela.

### 6.1 The subset

A `@field` function is a **total, pure, comptime-analyzable subset** of
ordinary wrela: no actors, no pools, no suspension, no unbounded loops
(comptime trip counts only; domain-repetition operators are the loop-free
idiom). A whole-tree sema pass **abstract-interprets** the function with
`p: Vec3` held symbolic, producing **FieldWir** — a DAG, not a token
expansion. This is the same shape as every other stage in the compiler, and
it means the marked subset plus one pass replaces what would otherwise be a
DSL, a parser, and a second toolchain.

```
@field
fn crate(p: Vec3, seed: u32, wear: f32) -> Sd:
    body  = box(vec3(0.5, 0.5, 0.5)).round(0.02)
    slat  = box(vec3(0.52, 0.06, 0.06))
    slats = slat.repeat(axis = Y, period = 0.14, count = 6)
    return body.subtract(slats, smooth = 0.01)
               .displace(fbm(p * 8.0 + rand(seed), octaves = 2) * 0.004 * wear)
```

### 6.2 Derived programs

One source, N derived programs. This table is the spec shape.

| Derived program | Derived by | Consumed by |
| --- | --- | --- |
| `eval` | direct lowering | fine marching, reference |
| `eval_packet4` / `_packet8h` | SoA transpose, fp32 / fp16 | ray packets (§3) |
| `eval_grad` | symbolic ∂ | normals, Newton steps (§2.5) |
| `eval_hess` | symbolic ∂² | curvature → edge wear, AA (§9) |
| `eval_range` | affine-arithmetic transform | tile classification, pruning (§2.1) |
| `eval_dir_lip` | local directional bound | segment tracing (§2.4) |
| `eval_dt` | ∂/∂t bound | time certificates (§4.2) |
| `tape` | flattened DAG as data | runtime pruning (§2.2, §7) |
| `cost` | FLOP model over the DAG | the budget diagnostic (§6.5) |

Exact analytic normals from `eval_grad` replace 4- or 6-tap finite
differencing: 4× cheaper and exact.

### 6.3 Runtime parameters carry ranges

FieldWir leaves for runtime values declare a range: `angle: f32 in 0.0..90.0`.
The range propagates through `eval_range`, so **the compiler bounds an
animated object's swept region at comptime.** That is what makes the
instance BVH refit-free, it feeds probe invalidation (§4.3), and it is what
`eval_dt` needs to bound `L_t`.

This is also the line the no-JIT rule draws, and it is a good one:

> **Field topology is comptime. Field parameters are runtime.**

with the single deliberate exception in §7.

### 6.4 Diagnostics

This is the [CLAUDE.md](../CLAUDE.md) "one place not to be dumb" clause,
and the authoring surface lives or dies on it. A field that lies about
distance breaks marching *silently*; make it loud.

```
error[FIELD_UNBOUNDED]: displace() amplitude 0.4 exceeds the Lipschitz
  margin at this node; the marcher will overstep.
  --> world/crate.wr:12
   |
12 |     .displace(fbm(p * 8.0) * 0.4)
   |               ^^^^^^^^^^^^^^^^^^ bound here is k = 12.6, margin needs ≤ 0.08
   |
  help: reduce the amplitude to 0.08, or annotate @lipschitz(k = 5.0) if the
        field is bounded by construction.
```

Companion fuzz lane: sample fields at random points, check `‖∇d‖ ≤ 1`,
report measured reach in the existing lane style.

### 6.5 The cost model is the frame budget

Extend the existing `CostRule` / `cost-inventory` machinery to FieldWir.
The compiler knows each shape's FLOP/eval, instance counts, resolution, the
pass graph, and core count. It can compute a static frame-cost bound and
**refuse to seal an image whose scene cannot make its frame**.

That is a build-time diagnostic for a problem every other studio discovers
with a profiler in week 47. Combined with a hard runtime step budget —
when the marcher exhausts it, fall back to the coarse pruned result — it
gives **guaranteed frame time with graceful degradation**, which follows
directly from the closed-world model and which no PC engine can offer.

Oracle: a `cost.txt` golden per scene.

---

## 7. The tape interpreter: ruling

**Ruled legal, deliberately.** Write this into the normative chapter so no
future session relitigates it.

A pruned tape is **data**, not code. Interpreting data is not loading code:
no codegen at runtime, no W^X, no dynamic loader, no foreign code. The
closed world of [01 §2](../docs/language/01-model.md) is intact.

**Boundary clause** — this is the part that must be explicit, because
"tapes are data" is otherwise a hole a future session widens until there is
a VM inside the VM:

- The interpreter may execute **field evaluation only**: arithmetic and the
  fixed FieldWir operator set over `Vec3 → f32`.
- It may **not** execute control flow, actor logic, message dispatch, or
  any general operation set. There is no tape opcode that calls, branches
  on runtime data, or allocates.
- It lives in `stdlib` as guest code. It is **not** a second compiler
  backend and does not appear in `crates/wrela-compiler`.
- Tape-interpreted `eval` is pinned **bit-identical** against compiled
  `eval` under `diff-eval`. This is a hard gate, not a smoke test.

**What this one mechanism resolves.** The case for the interpreter is not
only pruning:

1. **Pruning** (§2.2) — per-region specialization that compounds with depth.
2. **No JIT** — the closed world survives.
3. **Runtime topology mutation** — destruction is "append a CSG node."
   Without an interpreter that requires a fixed edit pool and a
   bake-invalidate path; with one, Teardown-class world mutability is
   native. (A bounded pool still bounds the *edit count*, which is the
   right place for the budget to live.)
4. **Backend quality** — the hot loop becomes ~200 instructions of
   hand-tuned wrela executed 10⁸ times. The backend must be good on **one
   loop**, not on arbitrary generated field code.

Point 4 is the one that decides it. Compiling a specialized packet marcher
per field is precisely the thing the "embarrassingly naive backend"
doctrine forbids — fixed frames, spill everything, every check emitted, and
a 5–10× penalty. That conflict is real and unresolved for the
specialization route. The interpreter converts an open-ended codegen
quality problem into a single bounded kernel that can be hand-tuned once
and locked with a bench threshold.

Cost: interpretation is ~2–3× per-op versus compiled. If pruning takes 500
ops to 15, the trade nets 10–15×. **This ratio is measured in §16, not
assumed** — and it is the one §16 metric the M4 proxy biases *optimistic*
(§16.1), because a wide out-of-order window hides exactly the dispatch cost
A76 would pay. Discount whatever the proxy reports; do not lock a threshold
on it before hardware.

---

## 8. Lighting

Cheapest first. Coherence is what reads as expensive; uncoordinated
features are what read as cheap.

| Layer | Mechanism | Cost |
| --- | --- | --- |
| AO | 4–5 distance samples along the normal | near-free, and it is most of what makes fields read as solid |
| Soft shadows | one sphere-traced ray to the light; penumbra from `min(k·d/t)` | one ray, no shadow map, physically-shaped softness, fp16 |
| GI | world-space irradiance probe clipmap, round-robin a few dozen per frame, short rays into the pruned field | amortized <1 eval/pixel/frame; invalidation per §4.3 |
| Emission | just another field | area lights and glowing geometry are native |
| Volumetrics | density fields marched at ⅛ resolution | the marcher is already stepping; most engines fake this |
| Sky | Bruneton precomputed scattering (§13) | ~1 MB of tables, near-zero runtime |

**Walk on Spheres** (Sawhney & Crane 2020) deserves a note, because it is
the most elegant idea in the space: it solves PDEs grid-free using
*distance queries as its only geometric primitive* — the identical
operation as sphere tracing, pointed at transport instead of visibility.
Sphere tracing and WoS being the same primitive is a strong signal the
representation is right.

It is **not** the real-time GI. It solves Laplace/Poisson — the diffusion
approximation — with no directional indirect and no correct indirect
occlusion, and as a Monte Carlo estimator it needs many walks per estimate,
each several distance queries. Position it for **subsurface scattering**,
and possibly for updating a handful of probe irradiances per frame. Not a
pillar.

---

## 9. Materials, detail, and the look

### 9.1 The signals fields give away

The whole argument for AAA-grade surfacing without assets:

| Signal | Source | Buys |
| --- | --- | --- |
| Exact normal | symbolic ∇ | cheaper and exact |
| Curvature | symbolic ∇² | **edge wear** — the single largest AAA-vs-amateur tell |
| Occlusion | 5 distance taps | **crevice grime**, contact grounding |
| Thickness | march inward | subsurface scattering |
| Up-facing-ness | ∇·up | water streaks, dust, snow |
| Coverage | screen-space `d` vs cone footprint | analytic silhouette AA |

Procedural weathering — edge wear, crevice dirt, gravity-aligned streaks,
rust bloom — is **more natural in a field renderer than in a rasterizer**,
because its inputs are analytic here and require baking or screen-space
hacks there. This is the strongest single lever on perceived quality per
FLOP in the whole document.

**Materials ride the blend.** `smin` blend weights are the material mixer:
where stone smins into moss, the materials crossfade with the geometry,
automatically and for free.

### 9.2 The footprint rule (mandatory, type-level)

**Every detail term takes the ray's cone footprint as an input and fades to
zero below the pixel.** Not a convention — a requirement of the stdlib
types, so a field whose cost the compiler cannot bound is unwritable.

Two consequences:

- **Shimmer becomes impossible by construction.** Temporal stability is the
  number one thing separating "polished" from "programmer art," and this is
  analytic mipmapping of procedural detail.
- **LOD is a special case of tape pruning.** Feed the cone radius into the
  pruning predicate and terms with amplitude below the footprint prune
  themselves at distance. No LOD authoring, no LOD data, no popping — it is
  a continuous bandlimit, not a discrete swap.

Detail terms should be constructed with a **known power spectrum** (Gabor
noise, Lagae et al.) so the marcher derives a sufficient sampling rate from
Nyquist rather than from worst-case Lipschitz.

### 9.3 The detail bands

"Looks like Shadertoy" means one frequency band populated. Three,
decorrelated:

1. **Silhouette** — the field itself.
2. **Mid-frequency** — displacement in the field. Costs Lipschitz margin;
   budget it (§6.4).
3. **Micro** — normal and roughness perturbation at shade time. Free; never
   touches the march.

### 9.4 The look layer is not optional

Most field renderers look cheap because they skip colour discipline, not
because fields look cheap. Exposure → filmic tonemap → computed grading LUT
→ dither (kills banding at 8-bit scanout and is period-appropriate
texture). At 720p this is bandwidth-bound, not compute-bound, and it is the
cheapest perceived quality in the frame.

Analytic AA from coverage handles **primary silhouettes only**. It does not
cover shading aliasing (that is 9.2's job) or thin-feature dropout. Right
technique; not a solved problem.

### 9.5 Art direction

Be clear-eyed: not Unreal photorealism — *craft coherence*. The medium's
native strengths are perfectly smooth surfaces, exact soft light, organic
blends, infinite procedural detail, and honest volumetrics. That is a
porcelain / clay / dreamlike palette. Give the console a look the way the
Game Boy and PS1 had looks, and enforce it in the stdlib. The target is
*Journey* / *Inside* / *Claybook*, and that look ages better anyway.

Faces are the honest hard part. Answer: stylized heads plus 2D field decals
(eyes, mouths) projected on, puppet aesthetics. Fonts likewise — a
geometric stroke/bezier SDF font authored in wrela source. Tables of
numbers in source are source, not assets.

---

## 10. Animation

The part most likely to make everything else read as amateur if it is
wrong. A sword swing has to look perfect.

### 10.1 Implicit skinning is the win, with one required operator

A limb is a tapered capsule *field* in bone space; "skinning" is evaluating
a blend of bone-space solids, not dragging vertices by weights. That
deletes the failure modes that consume most of a character artist's time:

| Mesh problem | Standard fix | Fields |
| --- | --- | --- |
| Candy-wrapper forearm twist | twist bones, correctives | absent — no vertex has two masters |
| Volume loss at elbow/knee | corrective blendshapes | `smin` **adds** volume at a bend, like flesh |
| Armpit self-intersection | weight painting | solids cannot interpenetrate |

Prior art: Vaillant et al., *Implicit Skinning* (SIGGRAPH 2013) — whose
expensive step is mesh→field→mesh conversion, which does not apply here.

**The required operator.** "Smin gives organic blends for free" is not
true at a folding joint — naive `smin` balloons. Use **gradient-based
blending** (Gourmel et al. 2013): the operator keys off the angle between
the two gradients, so near-parallel gradients blend smoothly and opposing
gradients produce a **contact bulge** rather than a merge. This is the
difference between flesh and balloon animals. One FieldWir node, in the rig
layer on day one.

### 10.2 Arc-first authoring

The trap is believing "pure math" means motion must *emerge* from formulas.
Oscillators and IK have no taste. What makes a swing read as perfect is
what hand animators control — the **arc** the blade tip traces and the
**timing** along it. Both are curves. Curves are source, not assets.

So invert the authoring: a swing is a spline for the blade-tip trajectory
in character space plus a speed profile over it. Two-bone IK solves the arm;
the pose is derived, not primary.

```
@clip(frames = 24, rate = 60)
fn slash_horizontal() -> Clip:
    arc  = spline3([ ... ])                                # blade-tip path, chest space
    pace = curve([(0, 0.0), (7, 0.05), (11, 0.85), (23, 1.0)])   # slow, SNAP, settle
    phase(windup = 0..7, active = 8..11, recover = 12..23)
    ik.hand_r = arc.at(pace)
```

Weight lives almost entirely in the speed profile. A dagger and a claymore
share an arc and differ only in time-remap. That factoring — *what* (arc,
key poses) separated from *when* (timing) — is the tunability requirement.

### 10.3 The twelve principles as operators

Anticipation = the arc extended backward in time with a pullback pose.
Follow-through = overshoot past the end key plus a damped spring settle.
Squash/stretch = a volume-preserving scale warp keyed to speed.

**Smear is where fields beat meshes outright.** AAA fakes fast swings with
hand-authored smear meshes. Here the blade during the active frames renders
as its **swept volume** — the field unioned over `[t−τ, t]` — which is
literally the smear frame animators draw, and it is a trivial 4D field
expression. For a pure rotation the swept capsule is a **torus segment,
closed form, one primitive.** True motion-blurred geometry is the specific
thing that makes a swing read as fast and heavy.

### 10.4 Layers

Evaluate a pose as a stack of pure functions of time, each independently
tunable:

1. **Base clip** — keyframed, frame-exact. Hand-tuned to perfection.
2. **Inertia** — critically-damped spring per bone, lagging the base pose.
   *This is where weight comes from.* A sword feels heavy because the blade
   trails the wrist by ~40 ms and overshoots on the stop. Two parameters per
   bone, and the highest-leverage tuning surface in the system.
3. **IK** — foot planting, hand-on-grip, look-at.
4. **Reaction** — hit-stop (freeze the time-remap 60–100 ms), recoil
   (blend to a rebound arc solved off the contact normal, which the field
   gives exactly), ragdoll blend.

Layering means tuning secondary motion never breaks the silhouette of the
swing. And because blended limbs never crack, poses can be pushed harder
than skinned meshes allow — wilder arcs, deeper crouches, more extreme
anticipation, which is where swings get their read.

### 10.5 The tuning loop is the feature

Beauty comes from iteration count, and this is where the determinism
doctrine pays off as a tool no studio has:

> **Record a combat encounter. Frame-step it. Change one spring constant.
> Recompile. Replay the identical input sequence and diff frame-by-frame.**

Requirements, treated as hard constraints on the compiler rather than
nice-to-haves:

- **Sub-second edit → pixels.** Non-negotiable; animation is tuned in
  hundreds of iterations.
- A **live parameter surface** in dev builds: curves and constants marked
  tunable, patched into the running VMM with scrubbing, onion-skin
  ghosting, and arc overlays drawn by the same field pipeline. This is data,
  not code, so the closed world survives — but the shipped image must be
  byte-identical to the tuned one, so the tuning surface **writes back to
  source** and the golden pins the source, never the live patch.
- Oracles: a pose transcript golden plus framebuffer goldens on **named
  frames** of the swing — the anticipation extreme, the contact frame, the
  settle. A tuned motion is an artifact; review it in the diff.

Combined with frame-exact timing (no OS, no frame pacing jitter, hard
deadline with graceful fallback per §6.5), this is **fighting-game-grade
combat as a structural property of the machine.**

---

## 11. Physics

Collision is free and exact. The world is a distance query: contact normal
is the gradient, penetration depth is `−d`.

- **Character controller:** `while d < r: p += n * (r - d)`, two or three
  iterations. No collision mesh, no convex decomposition, no broadphase
  against the environment, no mesh/render divergence bugs.
- **Rigid bodies:** query a comptime-sampled surface point cloud against the
  field. The point cloud is derived from the shape, so it cannot desync.
- **Water:** Gerstner heightfield (closed form) plus the above → boats,
  buoyancy, splashes.

This is *simpler* than a polygon engine, not harder.

---

## 12. The game thesis

"Fun as fuck" should come from verbs only this renderer can do. Not a
garnish — the design thesis.

- **Everything deforms.** Subtraction is one node. Carving, melting,
  welding, flooding. Teardown-class mutability, nearly free (§7).
- **Enemies merge and split.** `smin` is native. Creatures that fuse and
  divide.
- **Morphing.** Lerping between fields is trivial and looks like nothing a
  mesh engine can do — enemies that melt, doors that grow, weapons that
  flow between forms.
- **Scale-free zoom.** No LOD popping, so micro↔macro scale-shifting
  gameplay actually works.
- **Non-euclidean space.** Domain repetition and folding are nearly free.
  Portals, impossible rooms.

Destruction budget lives where budgets belong: a bounded edit pool, sized
by the compiler, enforced by [01 §2](../docs/language/01-model.md)'s pool rules.

---

## 13. What is precomputed, and what is not

**The principle:** the objectionable thing about a bake is not
precomputation, it is **stale persistent data** — a second representation
that can drift from the truth. The alternative is not "march harder," it is
**per-frame certificates**: proof objects computed at render time from the
live expression, valid for this frame, discarded after. §2 and §4 are that
family.

**Precomputed (~2 MB, "numerical constants, not bakes"):** definite
integrals with no closed form, which no runtime cleverness makes cheap.

| Table | Size | Why it cannot be live |
| --- | --- | --- |
| GGX split-sum BRDF LUT | 64 KB | numerical integration |
| Bruneton atmospheric scattering | ~1 MB | multiple-scattering integral; disproportionate quality/FLOP |
| Blue noise mask | 4 KB | offline optimization |
| Tileable noise volumes | small | optional; prefer analytic Gabor (§9.2) |

**Not precomputed: brick-baked distance fields.** For the record, and so a
bad argument does not get pinned here: **the memory objection is wrong.**
Sparse brick maps store surface-adjacent bricks only, and surface area
scales ~N², so a 512³-equivalent is ~5–20 MB, not the 128 MB a dense volume
would cost. Bricks fit on 1 GiB.

They are rejected on architectural grounds: **staleness** (a second
representation that drifts), **dynamism** (destruction and morphing require
re-bake and invalidation paths), and **redundancy** (tape pruning provides
per-region specialization without persistent state). If §16 shows
pathological static content that pruning handles badly, a brick cache may
return as a **bench-justified special case** decided by the cost model —
never by the semantics.

---

## 14. Prerequisites not yet designed

Neither is settled by a benchmark. Both block real work.

### 14.1 The bit-exact math library — **do this first**

Every golden-image oracle in this document — framebuffer goldens, pose
contact sheets, `diff-eval` between the comptime evaluator and the backend,
and the §7 tape-vs-compiled gate — requires `sin`, `exp`, `pow`, `sqrt`,
and every FMA-contraction decision to agree **bit-for-bit between the
host-side evaluator and aarch64 codegen**. Host libm will not.

Required: one softfloat/polynomial implementation, written in wrela, used
by both paths, with its own goldens. It constrains every field primitive
and is **unrecoverable if retrofitted**. It is also entirely in the spirit
of the stack — owning the whole thing includes owning the transcendentals.

### 14.2 The display path

`stdlib/drivers/` contains `blk.wr` and nothing else. There is no scanout
contract, no framebuffer device, no display driver. Every number in this
document assumes pixels can leave the machine.

The pinned host core settles part of this: **the host owns the panel**, so
scanout is a virtio-style handoff and "the Pi's HVS does the final scale
for free" is a *host-side* capability, not a guest one. §5 turns that from
a convenience into a requirement — the guest must hand off at internal
resolution, and something on the host side must scale 512×288 → 1280×720.

What remains open, and blocks renderer work:

- The internal→scanout **contract**: buffer format, count, and the
  synchronization discipline (the frame deadline in §6.5 is only real if
  the handoff has bounded latency).
- Whether the host-side scale is the HVS, a shader, or a CPU path on the
  pinned core — and what that costs, since the host core is already a
  tenant in L3 and on the bus (§1).
- Whether a **field-guided** upsample is possible across the boundary. At a
  2.67× scale factor the reconstruction quality matters a lot, and the
  guest holds the only oracle that can answer "are these two pixels the
  same surface." A dumb host-side bilinear throws that away.

---

## 15. Oracles

The verification map, in the [CLAUDE.md](../CLAUDE.md) table's own terms.

| What changes | Its oracle |
| --- | --- |
| A field primitive or combinator | `dump --stage=field` golden + a comptime-rendered 128×128 image golden |
| A FieldWir pass (grad, range, lipschitz) | that pass's dump golden |
| The tape interpreter | `diff-eval`: tape-interpreted vs compiled `eval`, **bit-identical** |
| A packet kernel | `diff-eval` against scalar `eval` |
| The marcher | framebuffer goldens on **named camera shots** |
| A material or weathering operator | framebuffer golden on a named material-ball shot |
| An animation clip | pose transcript golden + framebuffer goldens on named frames (anticipation, contact, settle) |
| A lighting stage | framebuffer golden, named shot |
| A field's cost | `cost.txt` golden |
| Lipschitz honesty | fuzz lane sampling `‖∇d‖ ≤ 1`, with measured reach |
| Renderer performance | a `bench` lane + a re-locked threshold |

Frames are deterministic by construction (fixed tile→core assignment, fixed
reduction order, §14.1's math), so **screenshots are goldens and the review
surface for graphics is a git diff.** No GPU engine has this.

---

## 16. The benchmark — what runs before any FieldWir is committed

Everything above that is not §1 rests on unmeasured ratios.

### 16.1 The M4 proxy — provenance, and what transfers

**No Pi 5 is available (2026-07-30).** The harness runs on an M4 MacBook
Air. Any number produced this way is **directional, not authoritative**,
and a future session must not cite an M4 run as settling a Pi 5 fact.

The M4 distorts precisely what §1 and §5 are about:

| | Pi 5 A76 | M4 P-core | Distortion |
| --- | --- | --- | --- |
| NEON pipes / core | 2 | 4 | ~2× |
| Clock | 2.4 GHz | ~4.4 GHz | ~1.8× |
| Memory bandwidth | ~8–9 GB/s | ~120 GB/s | **~14×** |
| L2 | 512 KB/core | 16 MB shared | **~30×** |
| Thermal | passive, steady | fanless, throttles | drifts under load |

So **wall-clock timing on this machine is close to worthless** for §1: a
bandwidth- or cache-bound design looks fine here and collapses there.

What survives the port is **counts**, because counts are properties of the
algorithm and the scene, not the microarchitecture:

| Metric | Transfers | Note |
| --- | --- | --- |
| Pruned tape length by depth | **yes** | pure algorithm |
| Interior-tile fraction | **yes** | pure algorithm |
| Blend-band ray fraction | **yes** | scene property |
| `map()`-equivalent evals/pixel | **yes** | a count |
| Reprojection hit rate, disocclusion area | **yes** | algorithmic |
| Interpreter overhead ratio | **biased optimistic** | M4's wide OoO window hides the dispatch cost A76 pays; discount it |
| Achieved GFLOP/s as % of peak | **no** | do not port this number |

**The §16.4 kill criterion is stated entirely in counts, so it is fully
evaluable on the proxy.** What the proxy cannot do is *set* the resolution
target — that stays at §1's modeled floor until hardware exists.

Two consequences for the harness design, both of which make it better than
a timing rig would have been:

1. **Emit counts and modeled cost, not measured time.** Instrument FLOP and
   byte-traffic counts, then apply §1's A76 cost model analytically. This is
   the `CostRule` discipline the project already runs on, and it converts an
   untransferable measurement into a transferable one.
2. **Pin the counts as a golden now.** They are deterministic, so a later
   Pi 5 run validates the port on counts and leaves timing as the *only*
   new variable — a one-variable experiment rather than a fresh
   investigation.

Secondary, cheap, clearly labelled: a macOS E-core-pinned timing (background
QoS) as a less-wrong proxy, reported as a distribution over short runs with
thermal state recorded, never as a mean.

### 16.2 Two scenes, not one

1. **Representative environment.** Hard-surface architecture with smoothed
   seams, moderate instance count, a static camera path. Tests the
   optimistic case.
2. **Worst case: a `smin` character cluster mid-swing, camera whipping.**
   This one scene stresses three assumptions simultaneously — `smin`
   clusters do not prune (overlapping blend bands defeat interval
   exclusion), blend bands force marching (defeating §2.3), and a whipping
   camera is reprojection's weakest case (§4.4). **This is the scene that
   will tell the truth**, and it is also the flagship gameplay case.

### 16.3 Instrument it like a fuzz lane

Print **measured reach**, so a collapse to a flattering number on an
unrepresentative scene is visible rather than silent:

- **Pruned tape length by subdivision depth.** Decides §2.2.
- **Fraction of screen area resolved as interior tiles** (no ray traced).
  Decides §2.1.
- **Ray-length fraction inside blend bands.** Decides §2.3 — whether
  "solve, don't march" is a pillar or a nice-to-have. Hard-surface scene
  might be 5%; the character cluster might be 40%. Neither of the two design
  passes that produced this document named this number, and it is the single
  most load-bearing unmeasured quantity here.
- **`map()`-equivalent evals per pixel**, and cycles/pixel. The harness has
  no game in it, so it will run on all 3 guest cores — **score it against
  the 2.4-core-equivalent budget** (§1), or it flatters itself by 25%.
- **Achieved GFLOP/s as a fraction of peak.** §1 assumes 30% sustained and
  every downstream number rests on it. If the real figure is 20%, the floor
  moves before any of the ratios below matter.
- **Interpreter overhead ratio** vs compiled eval, on the same tape.
- **Reprojection hit rate** and disocclusion area on the camera-whip case.

### 16.4 The decision it makes

If pruned tapes land at tens of ops, interior tiles dominate, and blend-band
fraction is low, the §1 floor of **512×288@30** is a floor and the surplus
buys resolution, framerate, or lighting. If they do not, the design does not
change shape — the resolution does, downward, to 480×270. **Do not commit
FieldWir before these numbers exist.**

Because §2 is load-bearing rather than upside at 2.4 render cores (§1), add
a **kill criterion**: if the worst-case scene shows pruned tapes above ~100
ops at depth 3, interior-tile fraction under ~50%, and blend-band ray
fraction above ~30% together, the 512×288 floor is the ceiling too, and the
honest response is to revisit the constraint set with the human rather than
to keep optimizing.

---

## 17. Decided — do not relitigate

1. The authoring language is **wrela**, via the `@field` subset and
   abstract interpretation into FieldWir. No shader sublanguage.
2. Field **topology is comptime; parameters are runtime** — with §7's tape
   as the single, bounded exception.
3. The **tape interpreter is legal**, under §7's boundary clause and its
   bit-identical `diff-eval` gate.
4. The **console owns the renderer**; games author `field`, `mat`, `anim`,
   and game logic only. Games declare *what*; the console owns *how*, so
   rendering improves for every shipped title on the next image recompile.
5. No bakes; ~2 MB of numerical constants (§13). Bricks return only as a
   bench-justified special case.
6. fp16 is a **precision tier in the IR**, with marching state fp32 (§3).
7. The **footprint rule is mandatory and type-level** (§9.2).
8. **Gradient-based blending** in the rig from day one (§10.1).
9. **Tile fusion; no full-frame intermediates** (§5).
10. The **cost model refuses to seal an over-budget image** (§6.5).

## 18. Rejected, with reasons

| Rejected | Why |
| --- | --- |
| Naive per-pixel sphere tracing | out by ~10× (§1); not rescuable by resolution |
| Brick-baked distance fields as a pillar | staleness, dynamism, redundancy — **not** memory (§13) |
| Comptime path-traced irradiance | freezes the world; probes converge live instead |
| Walk on Spheres as the real-time GI | diffusion approximation, high variance; keep it for SSS (§8) |
| Per-field specialized packet kernels | conflicts with the naive-backend doctrine at 5–10×; §7 point 4 |
| Neural SDFs | an asset by another name, and CPU-hostile |
| Time-Lipschitz certificates as the *default* temporal scheme | heavier than the rigid-instance case needs (§4.1) |
| A separate shader language | a DSL, a parser, and a toolchain to maintain for no gain (§6.1) |

## 19. References

- Keeter, *Massively Parallel Rendering of Complex Closed-Form Implicit Surfaces*, SIGGRAPH 2020 — tape pruning.
- Galin, Guérin, Paris, Peytavie, *Segment Tracing Using Local Lipschitz Bounds*, Eurographics 2020.
- Gillespie, Yin, Crane, *Ray Tracing Harmonic Functions*, SIGGRAPH 2024 — Harnack tracing.
- Sawhney & Crane, *Monte Carlo Geometry Processing*, SIGGRAPH 2020 — walk on spheres.
- Keinert et al., *Enhanced Sphere Tracing*, 2014 — over-relaxation.
- Comba & Stolfi, affine arithmetic; Fryazinov, Pasko, Comninos, revised AA for implicit ray tracing.
- Vaillant et al., *Implicit Skinning*, SIGGRAPH 2013.
- Gourmel et al., *A Gradient-Based Implicit Blend*, ACM TOG 2013 — the contact-bulge operator.
- Lagae et al., Gabor noise — band-limited procedural detail.
- Bruneton & Neyret, precomputed atmospheric scattering.
- Laine & Karras, *Efficient Sparse Voxel Octrees*, 2010 — beam optimization, the ancestor of §2.1.
