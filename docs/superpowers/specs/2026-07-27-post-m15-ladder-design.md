# Post-M15 ladder: stdlib maturity, entropy, perf deferred

**Status:** design approved in conversation (2026-07-27); not an activated
milestone plan. `plans/M16.md` / `plans/M17.md` are written only when those
rungs activate, per ROADMAP planning convention.

**Context:** M15 (variable cores + true concurrent vCPUs) is ACTIVE. Today's
ROADMAP places the cycle proxy at M16 and the optimization playground at
M17. This design inserts two rungs before that perf track and renumbers
accordingly. Pixels (input + display + compositor) stays an unscheduled
intention after the renumbered perf rungs.

## Goal

Make the stdlib and machine device story honest and testable before spending
the cleverness budget:

1. Relocate real `@driver` implementations into `stdlib/drivers/`.
2. Add an in-wrela coverage suite for `core` (and runtime pins for drivers).
3. Land entropy as a thin, recorded device — not a `@driver`.
4. Fix normative overclaims that every machine-v1 device has a stdlib
   `@driver` and that every row is "virtio."
5. Push cycle proxy / optimization shelf later (M18 / M19).

## Vocabulary (settled — do not blur)

Two independent axes:

| Term | Means |
| --- | --- |
| **Device** | VMM model + report/conformance (+ record/replay as applicable). Rows in the closed machine-v1 set. |
| **`@driver`** | Guest actor root that owns a multiphasic device protocol (pools/MMIO/DMA, receipts, optional mailbox). Language construct from 03. |
| **Thin guest surface** | Sealed language/runtime API for devices that do not earn a `@driver` (`now()`, console ring helpers, entropy effect). |
| **`stdlib/drivers/`** | Package tree containing **only** `@driver` modules. |

All machine-v1 rows are devices. Only some necessitate a full `@driver`.
Say **virtio** only where the shipped contract is actually virtio.

### When `@driver` earns its keep

Use `@driver` when there is multiphasic protocol / reset epochs, DMA
ownership, and a client messaging surface. Do not use it for abort-path
logging, monotonic clock reads, or "give me N random bytes."

| Device | `@driver`? | Why |
| --- | --- | --- |
| `blk` | yes | virtio-blk queues, DMA, receipts, reset |
| `input` / `display` | yes (pixels rung) | real protocol + clients |
| `clock` | no | sealed `now()` → trapping MMIO; deadlines depend on direct reads |
| `console` | no | fixed ring + VMM drain; must serve panic/abort without actor turns |
| `entropy` | no | recorded nondeterministic bytes; clock-shaped, not blk-shaped |

## Normative device split (06 §6 rewrite)

M16 opens with a human-reviewed 06 edit (and matching ROADMAP / stdlib
README / ledger note fixes). Replace the single "each … whose driver ships
in the stdlib" claim with two subsections:

### Thin device contracts (no `@driver`)

| Device | Contract | Guest surface |
| --- | --- | --- |
| `clock` | trapping monotonic MMIO | `now()` / `core.time` |
| `console` | fixed console-ring + VMM drain (**not** virtio-console) | runtime / optional `core` helpers |
| `entropy` | recorded entropy source (**not** virtio-rng rings) | sealed runtime effect (lands M17) |

### Queue device contracts (`@driver` in `stdlib/drivers/`)

| Device | Contract | Guest surface |
| --- | --- | --- |
| `blk` | **virtio-blk** (as shipped by M7) | `drivers.blk` |
| `input` | pixels rung | `drivers.*` |
| `display` | pixels rung | `drivers.*` |

Future revisions (`net`, `sound`) stay outside machine-v1 conformance per
M13 item D; when revised they are expected to be queue/driver-class unless
a later design says otherwise.

**Blk transport:** keep virtio-blk for now (M7 paid for it; vDPA / unchanged
guest under alternate backings remains plausible). A trimmed `wrela-blk` is
a separate machine-revision conversation — not M16/M17. Thin devices cheat
freely on transport because both sides are owned; queue devices still need
shared rings + doorbells for the zero-exit hot path (06 §5), whether branded
virtio or not.

## Ladder

```text
M15 (ACTIVE) → M16 Stdlib maturity → M17 Entropy
             → M18 Cycle proxy → M19 Opt shelf
             → pixels (intention)
```

ROADMAP and ledger owner strings that today say M16/M17 for proxy/shelf are
renumbered to M18/M19 in the same doc commit family that inserts the new
rungs. Plans for M16/M17 are **not** written until activation.

### M16 — Stdlib maturity

**Owns:**

1. **06 honesty rewrite** (device split + virtio-only-where-true), plus
   stdlib README / ROADMAP echo fixes.
2. **`stdlib/drivers/`** as sibling of `stdlib/core/`, imported via the
   existing package-root loader rule (e.g. `from drivers.blk import …`).
3. **Relocate blk** out of inlined golden `@driver` bodies into
   `drivers.blk`. Existing boot transcripts remain the oracle (byte-identical
   or deliberately re-pinned with a ledger cite). Move, don't redesign.
4. **Dual-tier in-wrela suite:**
   - *Comptime:* `@test` / `@test(exhaustive)` over pure `core` (List,
     SlotMap, time constructors, Format helpers, Result/`from`, etc.). No VMM.
   - *Runtime:* small `@test(runtime)` images that import `drivers.blk` and
     pin transcripts (prove the new import path, not only the old inlined
     bodies).
5. Wire the suite into `cargo xtask check` as a named lane (exact name frozen
   in the plan). Empty suite root fails closed.
6. **Console / clock:** name them as thin contracts; floor cleanup only if
   required for honesty. No console `@driver`. No clock `@driver`.

**Explicit non-goals:** entropy; input/display; cleverness-budget spends;
KVM; replacing virtio-blk; inventing a general device framework.

**Exit criteria (coarse; plan freezes the walk):**

- No inlined `@driver` bodies left in the blk boot goldens M16 owns.
- Comptime stdlib suite green under `check`.
- At least one runtime golden imports `drivers.blk` and matches its pin.
- 06/README/ROADMAP no longer claim every device has a stdlib `@driver` or
  that console/entropy are virtio.
- Ledger clauses for packaging + suite opened/flipped as the plan names.

### M17 — Entropy

**Owns:**

1. Thin entropy **device** in the VMM: recorded-source model (live host
   entropy; replay from the choice log; fail closed on underrun — clock
   underrun is the precedent).
2. Sealed guest runtime effect (name/API frozen in the plan; roughly
   `entropy(n) -> Bytes`), illegal at comptime/ISR like `now()`, lowered to
   a small fixed machine contract (trapping MMIO or equally small path — **no**
   virtqueues required for v1).
3. Boot + replay golden that diverges if bytes are not logged/replayed.
4. Normative row under thin device contracts (if not already placed by M16's
   06 split with entropy marked "lands M17").

**Not a `@driver`.** Does not add modules under `stdlib/drivers/`.

**Why its own rung:** new VMM model + guest intrinsic + recorder path +
conformance golden is still a real surface; M16's packaging/test exit must
not wait on it. Folding into M16 remains a human call at activation if
entropy is truly tiny — default is split.

**Explicit non-goals:** input/display; net/sound; virtio-rng rings;
`stdlib/drivers/` changes.

### M18 — Cycle proxy / M19 — Optimization playground

Substance unchanged from today's ROADMAP M16/M17. Renumbered only.

### Pixels (intention, after M19)

Input + display + compositor + golden frames. Still human-gated. When
scheduled, these are queue/driver-class devices under `stdlib/drivers/`.

## Stdlib layout (M16)

```text
stdlib/
  core/           # existing prelude / collections / time / runtime surface
  drivers/        # @driver modules only
    blk.wr        # (or blk/… if it outgrows one file)
  README.md       # stops claiming a complete driver set it does not ship
```

Console and entropy helpers, if any, stay under `core` or runtime — never
under `drivers/` unless they become true `@driver`s (rejected for v1).

## Testing

| Tier | Runs | Covers |
| --- | --- | --- |
| Comptime `@test` / `@test(exhaustive)` | `wrela test`, no VMM | pure `core` |
| `@test(runtime)` importing `drivers.*` | VMM boot goldens | relocated drivers |
| Existing boot/err goldens | as today | language/machine oracles; bodies updated to import drivers where M16 relocates them |

Harness: a `check`-wired lane so absence of stdlib coverage is a gate
failure, not a README hope. Exact discovery root (`stdlib/**` vs
`stdlib/tests/`) is a plan decision; fail closed if empty.

## Doc / ledger motion (same-commit discipline)

When M16 activates, the first commits include:

- 06 §6 split (thin vs queue; virtio only where true).
- ROADMAP: insert M16/M17; renumber proxy→M18, shelf→M19; active-plan
  pointer; recorded-intentions pointers that cited old numbers.
- `stdlib/README.md` alignment.
- Ledger: open/flip clauses the plan names; retarget owner strings that
  said M16/M17 for costs/playground.

M17 adds/flips entropy clauses and the guest-effect surface in 05/06 as
needed.

## Non-goals (whole design)

- Activating pixels.
- Making console, clock, or entropy into `@driver`s.
- Replacing virtio-blk with a custom queue ABI in these rungs.
- A plugin/generic device framework.
- Cleverness-budget optimizations (proxy/shelf come after).
- KVM / flagship-host bring-up.
- Writing `plans/M16.md` / `plans/M17.md` before those rungs activate.

## Rejected alternatives (do not relitigate)

1. **One fat rung** owning packaging + tests + entropy — rejected; packaging
   exit must not wait on rng.
2. **Keep M16/M17 numbers on perf** (M15b-style side names) — rejected;
   renumber for ladder clarity.
3. **Full machine-v1 including input/display before perf** — rejected;
   pixels stays afterwards.
4. **Console as `@driver`** — rejected as overbuilt; abort/panic path cannot
   depend on actor turns.
5. **Entropy as `@driver` / virtio-rng rings** — rejected; recorded thin
   source matches "small recorded-source model" already in ROADMAP.
6. **Force every §6 row to be a `@driver` for vocabulary uniformity** —
   rejected; protects the word `@driver` instead.
7. **Narrow "device" to only `@driver`-backed things** — rejected; VMM and
   conformance still model clock/console/entropy.

## Open points for the activation plans (not blocked)

These are deliberately plan-time, not design-time:

- Exact entropy effect name and lowering (MMIO address vs other small path).
- Exact `xtask` lane name and stdlib test discovery root.
- Whether console floor gains small `core` helpers in M16 or stays
  runtime-only.
- Whether M16 re-pins any blk transcript (ledger cite) vs byte-identical
  move.
- At M17 activation: confirm entropy still warrants its own rung vs fold.

## Success

After M17 closes (and M15 before it):

- `stdlib/drivers/` is the home of blk; goldens import it.
- In-wrela comptime suite covers pure `core` under `check`.
- 06 tells the truth about devices vs `@driver`s and about virtio.
- Entropy is a replayable thin device with a guest effect and a golden.
- Cycle proxy / opt shelf are next (M18/M19), then pixels when scheduled.
