# M12 Wave 7a — adversarial sweep findings

Worktree: `/Users/ryanwible/projects/wrela8/.worktrees/m12-adversarial`,
branch `m12-adversarial`, base `c7e050b` (items 0, A–G landed; item H
unowned by this sweep).

Orchestrator: verify each claim independently. Severity: **blocker**
(item H cannot honestly close), **high**, **medium**, **low**, **info**.

This sweep does **not** mark M12 COMPLETE and does **not** activate M13.

---

## Disposition summary

| Bucket | IDs |
| --- | --- |
| **Blockers fixed** | A-06 (ledger `sema.bounds.loops` M13→M15 slip) |
| **Deferred** (known / out of scope) | A-07, A-08, A-09, A-10 |
| **Not absorbed** | A-11 (broader ledger ladder-number drift outside the four exit clauses) |
| **Exit criteria** | A-01…A-05 — all mechanically green; no H-blocking defect found |

No representation regression (data ladders / numbered `RING{i}_` /
`WAKE_PEND` / `INIT_SLOT` statics) and no boot-transcript drift beyond
the intentional `boot-group-four-children` add.

---

## 1. Hunt lanes

### A-01 · **info** · Boot transcripts byte-identical to pre-rung

**Claim:** Every `tests/golden/boot-*/expected/test.txt` matches
`0fde6e5` (M12 activation) except the new
`boot-group-four-children` golden.

**Evidence:** `git diff --name-only 0fde6e5 HEAD --
'tests/golden/boot-*/expected/test.txt'` → only
`boot-group-four-children/expected/test.txt` (Added). Spot sha1 match
on `boot-hello`, `boot-actors`, `boot-send`, `boot-group-join`,
`boot-cross-core`. Non-boot drift:
`check-import-lower/expected/test.txt` gained `(rings padding 0)` in
the `RTDATA_SIZE_MAX` diagnostic (item C; expected).

**Owner:** H close checklist (cite).

### A-02 · **info** · No leftover data ladders

**Claim:** `__wrela_ring_get_head` / `__wrela_wake_pending_load` /
`RING{i}_CTL` / `WAKE_PEND*` / `INIT_SLOT*` are absent from
`rtconfig` emit, `runtime.wr`, and golden `rtconfig.txt` statics.
Fact ladders (`__wrela_ring_capacity` / `slot_words` / cores / kind /
handle) and init-span dispatch (`__wrela_init_*` over `INIT_SPAN*`)
remain — plan-allowed.

**Evidence:** `rg` over worktree (excluding `plans/` / `ROADMAP.md` /
`target/`) for the deleted accessor names → empty. Golden
`pub static RING[0-9]+_|WAKE_PEND|INIT_SLOT` → empty. Unit test
`golden_rtconfigs_forbid_numbered_ring_wake_init_slot_statics`
(`placed_static_census.rs`).

**Owner:** H close checklist.

### A-03 · **info** · Census bypasses are locked (with one sparse caveat)

**Claim:** `N` cannot grow past `FIXED_SET_LEN + spans` on a full fixed
set without failing the ratchet; `RING{i}_` resurrection fails the
rtconfig forbidden-name scan; names outside the closed set fail the
report closed-set scan. Adding a fixed name requires editing
`FIXED_CORE_NAMES` / `MB_POOL_COUNT`, which bumps `FIXED_SET_LEN`
(intentional).

**Caveat (not a bypass of the golden lock):** a lone unexpected name on
a *sparse* image does not break `N ≤ FIXED_SET_LEN + spans` by itself
— documented in `unexpected_name_on_full_fixed_set_breaks_the_ratchet`
(`placed_static_census.rs:269–273`). The closed-set golden scan is the
companion lock.

**Evidence:** `placed_static_census.rs` (`FIXED_SET_LEN = 79`,
`is_forbidden_rtconfig_static_name`, golden report/rtconfig tests);
ledger `runtime.rtdata.placed-static-census` note.

**Owner:** none (working as designed). Record caveat at H if exit prose
mentions only the numeric ratchet.

### A-04 · **info** · `RTDATA_SIZE_MAX` still fail-closed with padding number

**Claim:** `steer_rtdata_base` rejects `tables.total_bytes >
RTDATA_SIZE_MAX` with
`rtdata needs {total} bytes (rings padding {p}), which exceeds
RTDATA_SIZE_MAX ({max})`. Padding is folded into `total_bytes` via
`add_cross_core_rings`. No silent overflow path found.

**Evidence:** `layout.rs:517–524`, `:3850–3854`; pinned by
`golden/check-import-lower` (`rings padding 0` on a mailbox blowup).
Ring-bearing reports print
`Rings count={} stride={} padding={} bytes={}` (e.g.
`boot-cross-core-admission-order` `padding=624`). Peak golden `rtdata`
observed: 6664 (`boot-receipt-handoff`) ≪ 262144.

**Owner:** H close checklist. (No golden exercises `padding > 0` on the
*fail* path — **info**, not a defect; the format string is shared.)

### A-05 · **info** · Doc/ledger exit clauses present (one slip fixed)

| Clause | Expected | Measured |
| --- | --- | --- |
| `sema.bounds.loops` note | updated for const-name bound | **yes** (M12 item B paragraph); status stays `gap` (honest — async/ISR halves) |
| `actors.groups.child-capacity` | `test` | **`test`**, golden `boot-group-four-children` |
| `runtime.rtdata.placed-static-census` | `test` | **`test`** |
| `compiler.rtconfig.facts-only` | data-ladder sentence | **yes**: "a generated dispatch ladder is forced; a generated data ladder is a defect" |

**Slip fixed (A-06):** the Still-gap sentence said cycle proofs are
"deliberately not M13's proxy"; after the 2026-07-26 ladder, M13 is
vocabulary and the cycle proxy is **M15**. Corrected in-tree to
"not M15's cycle proxy … M13 is vocabulary".

**Docs:** 02 §8.1 / §13 carry the const-name wording; 02 §9.5
`capacity=4` example remains and is runnable.

**Owner:** H for the table; A-06 landed in this sweep.

### A-06 · **low** · Fixed · `sema.bounds.loops` milestone misnumber

**Claim:** Ledger note referred to "M13's proxy" while M12's own plan
and ROADMAP place the cycle proxy at M15.

**Evidence:** `ledger/ledger.toml` `sema.bounds.loops` note (Still-gap
paragraph); `plans/M12.md` ladder §; `plans/M13.md` Status DRAFT
(vocabulary).

**Fix:** wording → M15 cycle proxy; parenthetical clarifies M13 =
vocabulary. No golden (ledger prose only). Clause id:
`sema.bounds.loops`.

---

## 2. Deferred (do not expand scope)

### A-07 · **medium** · Stub `PlacedStatic` addr/size for overlays

**Claim:** Report `PlacedStatic` lines for some reinjected overlays still
show stub/high-zone addresses and floor sizes. Sharp example:
`boot-group-four-children` live rtconfig has
`GROUP_MAX_CHILDREN=4`, `GROUP_SLOT_SIZE=128`,
`@placed(0x405418f0) GROUPS`, but the report prints
`PlacedStatic name=GROUPS … addr=0x4057f778 size=96` (floor). Guest
path uses the reinjected 128-byte `GroupSlot` (transcript ok); the lie
is report/`collect_placed_statics` over stub-typed programs.

**Evidence:** `wrela dump --stage=rtconfig` vs `--stage=report` on
`tests/golden/boot-group-four-children/input.wr`;
`layout.rs:collect_placed_statics`; M12 non-goals / known residual
("stub PlacedStatic addrs").

**Owner:** **deferred** — do not expand this sweep. Optional follow-on:
collect placed statics from post-reinject programs, or publish sizes
from `RuntimeTables` facts.

### A-08 · **low** · MB pool high-zone stride (32 × 64)

**Claim:** `MB_POOL_COUNT = 32` overlays still occupy a 64-byte
high-zone stride each when unused; mailbox overlay convergence beyond
the census bound is an M12 non-goal.

**Evidence:** `rtconfig.rs:144`, `:736–748`; `plans/M12.md` Non-goals.

**Owner:** **deferred** (recorded, not scheduled).

### A-09 · **low** · `INIT_SPAN` import pool placeholders

**Claim:** Generator still emits `INIT_SPAN0..7` so `runtime.wr` can
import every overlay; unused slots are 1-word high-zone placeholders,
excluded from census `N` (item E residual option b).

**Evidence:** `placed_static_census.rs` module docs; ledger census note.

**Owner:** **deferred** (locked by census counting rule).

### A-10 · **info** · Dispatch ladders / trampoline pools remain

**Claim:** `__wrela_xsend_0..7`, ring *fact* match ladders, and
`__wrela_init_*` over live spans remain. Plan doctrine: dispatch
ladders forced; data ladders defects. `RING_POOL_COUNT` caps trampolines
post-C.

**Evidence:** `runtime.wr` imports; `rtconfig.rs` emit; facts-only note.

**Owner:** none (in doctrine).

---

## 3. Not absorbed

### A-11 · **low** · Broader ledger ladder-number drift

**Claim:** Other ledger notes still narrate pre-split / mid-renumber
ladders (e.g. "M13 the cycle proxy", "cycle-proxy milestone (now M12)",
"M12 authoring hardening") outside the four M12 exit clauses. Not
rewritten here.

**Evidence:** `ledger/ledger.toml` around the wait-for-graph /
cost-profile clauses (historical notes).

**Owner:** ledger hygiene when those clauses next move — not M12 H.

---

## 4. Exit criteria (mechanical)

From `plans/M12.md` "Exit criteria (mechanically checkable)":

| Criterion | Result |
| --- | --- |
| `cargo xtask check` green | run as gate for any fix commit; see close evidence |
| Placed-static census line + ratchet; no `RING{i}_` / `WAKE_PEND` / `INIT_SLOT` statics | **pass** (unit tests + golden scan) |
| Ring padding printed; no golden exceeds `RTDATA_SIZE_MAX` | **pass** (ring images have `Rings … padding=`; peak rtdata 6664) |
| `boot-group-four-children` pins capacity=4 | **pass** |
| Ledger flips / notes above | **pass** (A-06 note correction) |
| Boot transcripts == pre-rung | **pass** (A-01) |

Bench locks: `boot_actors_median_us = 700000` unchanged in
`bench/thresholds.toml`. Layout-assert code-size re-lock not required
by this sweep (no measured breach chased here).

Item **H** remains open for the orchestrator (evidence block, ROADMAP
COMPLETE, M13 activation on its own commit).

---

## 5. Commits from this sweep

1. Ledger: `sema.bounds.loops` Still-gap sentence — M15 cycle proxy
   (not M13); cite `sema.bounds.loops`.
2. This findings file.

No representation / golden / runtime behavior changes beyond the
ledger prose fix.
