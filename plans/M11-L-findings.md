# M11 item L — adversarial sweep findings

**Close disposition (item M, 2026-07-26, decisions 865–869):** blockers
L-02 / L-04 / L-05 / L-06 / L-07 / L-16 / L-20 addressed by exit-prose
amendments, ledger note 858, and measured bench/soak — see
`plans/M11.md` item M evidence. Deferred post-M11: L-11, L-15, L-19.
L-09 opening census labelled historical F0. IMAGE_STATIC remains 0;
`build_entry_driver` remains deleted. M12/M13 stay DRAFT.

Worktree: `/Users/ryanwible/projects/wrela8/.worktrees/m11-item-l`, branch
`m11-item-l`, base `55da7c3` (post items 0–K; IMAGE_STATIC=0, FLOOR=52,
Unclassified=23, GRAND=90).

Orchestrator: verify each claim independently. Severity: **blocker** (item M
cannot honestly close), **high**, **medium**, **low**, **info**.

Decision band for freezes if needed: **860–864** (865–869 reserved for M).
Prefer amending exit criteria / ledger notes over new freezes unless a
behavioral rule must change.

---

## 1. Exit criteria vs measured reality

### L-01 · **info** · `IMAGE_STATIC_SUM_OF_ROWS == 0` holds

**Claim:** Exit criterion (`plans/M11.md:919-921`) and live census agree:
`IMAGE_STATIC_SUM_OF_ROWS = 0` (`emitted_a64_census.rs:249`); no ImageStatic
rows remain in `EMITTED_A64_ENTRIES`; F0 live-count test green.

**Evidence:** `plans/M11.md:919-921`; `emitted_a64_census.rs:166-194`, `:249`,
`:650-654`; `cargo test -p wrela-compiler --lib emitted_a64` (3/3 ok,
2026-07-26).

**Owner:** **M** close checklist (cite measured 0).

### L-02 · **blocker** · Exit criterion “FLOOR_WORDS unchanged at 26” is false

**Claim:** Exit text requires `FLOOR_WORDS` unchanged at **26**, or growth
only by words *formerly embedded in ImageStatic bodies*
(`plans/M11.md:922-924`). Live lock is **52** (`emitted_a64_census.rs:247`,
test `:665-670`). Trail: 26 → 31 (+5 secondary SP, H/811) → 36 (+5 checkpoint
LR, I/821) → **52** (+16 primary trampoline, K/852). The +10 from H/I match
the ImageStatic-extraction carve-out; the **+16 trampoline was extracted from
Unclassified `build_entry_driver`**, not ImageStatic — so the letter of the
exit bullet fails even if the spirit (no new capability) holds under
decisions 852/855.

**Evidence:** `plans/M11.md:922-924`, `:714-717`, `:732-735`;
`emitted_a64_census.rs:222-247`, `:665-670`.

**Owner:** **M** — rewrite the FLOOR bullet to measured 52 + named extraction
trail (811/821/852), or reject the trampoline categorization.

### L-03 · **info** · `build_entry_driver` deleted; Unclassified is 23

**Claim:** No `fn build_entry_driver` remains; census Unclassified = 4
(`push_load_imm`) + 19 (`push_abort_tail` cfg(test)) = 23; GRAND = 90.

**Evidence:** `rg 'fn build_entry_driver'` empty under `crates/`;
`emitted_a64_census.rs:253`, `:259`; item K done note `plans/M11.md:900-906`.

**Owner:** **M** close checklist.

### L-04 · **blocker** · Exit “`runtime.wr` reachable via dump stages” fails as written

**Claim:** Exit requires algorithms in `runtime.wr` reachable by
`wrela dump --stage={typed,flowwir,mwir,asm}` like any other source
(`plans/M11.md:930-931`). On `tests/golden/boot-actors/input.wr`, those four
dumps succeed but contain **zero** `__wrela_rt_run_one` /
`__wrela_rt_enqueue` / `__wrela_rt_primary_entry` (and siblings). Dump asm
only force-roots `RUNTIME_FORCE_ROOT_KEYS` (probe/console/abort —
`lower.rs:305-322`). Migrated scheduler bodies enter the emit set only via
`codegen_runtime_force_roots_with` / `reinject_runtime_with_rtconfig` at
**layout** (`layout/harness.rs:1147-1216`, `:1359+`). Bare
`wrela dump --stage=typed stdlib/core/runtime.wr` **panics**
(`imports.rs:234` — `core.__image_runtime` not loaded). No golden pins
`__wrela_rt_run_one` under `expected/{typed,flowwir,mwir,asm}.txt`.

**Evidence:** dump probes 2026-07-26 (worktree `target/debug/wrela`);
`lower.rs:305-322`; `layout/harness.rs:1147-1216`; `plans/M11.md:930-931`;
glob search for `__wrela_rt_run_one` in dump-stage goldens → 0 files.

**Owner:** **M** — amend exit to “layout/image path + reinject” (honest), or
extend dump/`RUNTIME_FORCE_ROOT_KEYS` so dump stages actually surface the
migrated algorithms (and pin at least one golden per stage).

### L-05 · **blocker** · `bench guest` post-M delta not recorded

**Claim:** Exit requires item-0 baseline vs **post-M median** as a known
number (`plans/M11.md:935-937`). Only the before half exists: median
**63639us** (decision 720, evidence block `:980-989`). No post-K / post-M
guest median appears in the plan. Decision 839’s ~1377ms figure is the
**compiler build** lock, not guest.

**Evidence:** `plans/M11.md:208-213`, `:688-691`, `:935-937`, `:980-989`.

**Owner:** **M** — measure and record the delta (or disclose N/A with lock
still green) before COMPLETE.

### L-06 · **info** · `plans/M12.md` absent in this worktree (literal exit OK)

**Claim:** Exit / item M require `plans/M12.md` does not exist yet
(`plans/M11.md:912-914`, `:941`). On `m11-item-l` at `55da7c3`, neither
`plans/M12.md` nor `plans/M13.md` is present. ROADMAP on this branch still
names M12 as authoring hardening and says the plan “does not exist until
then” (`ROADMAP.md:548-580`).

**Evidence:** `ls plans/M12.md` → missing; `ROADMAP.md:548-580`.

**Owner:** **M** — keep absent at close, **or** amend exit if drafts are
intentionally landed (see L-07).

### L-07 · **high** · Main-tree drafts + exit/ladder wording drift

**Claim:** Untracked drafts exist on the human’s main tree at
`/Users/ryanwible/projects/wrela8/plans/M12.md` and `M13.md` (DRAFT —
PROPOSED; representation / vocabulary split). Closing M11 with those files
in the same tree would falsify the exit bullet unless M amends it. Exit
prose still pairs “hardening; cycle proxy is M13”
(`plans/M11.md:941`) — coherent with **this** worktree’s ROADMAP, but
fragile if the main-tree ladder draft renumbers M12 to “representation”.

**Evidence:** main-tree file presence + headers (2026-07-26); 
`plans/M11.md:912-914`, `:941`.

**Owner:** **M** — decide: keep “does not exist” and leave drafts
untracked/out of the close commit, or rewrite the exit bullet to allow
DRAFT plans that bind nothing.

---

## 2. Silent ImageStatic / floor / NON_INVENTORY growth

### L-08 · **info** · No silent ImageStatic REF rows; closed-set green

**Claim:** Zero ImageStatic inventory rows; `emitted_a64_hand_emitter_set_is_closed`
and enc-site lock green (codegen 470 / layout 8 / harness 41). NON_INVENTORY
lists inject stubs (boot/checkpoint/test/method) under decisions 812/823/831/851.

**Evidence:** `emitted_a64_census.rs:249`, `:279-283`, `:563-593`; tests green.

**Owner:** orchestrator re-run before M.

### L-09 · **medium** · Opening “Where this milestone starts” census table is stale

**Claim:** Plan header still shows ImageStatic **714** / Unclassified **117** /
Grand **872** as the F0 starting point (`plans/M11.md:32-44`) without a
“current after K” mirror. Not a code bug; easy for M evidence to cite the
wrong numbers.

**Evidence:** `plans/M11.md:32-44` vs `emitted_a64_census.rs:247-259`.

**Owner:** **M** evidence block (refresh or mark historical).

---

## 3. Facts-only generator (decision 702)

### L-10 · **info** · Facts-only + determinism unit tests green; no loops in goldens

**Claim:** `rtconfig::tests::facts_only_forbids_control_and_actors` and
`generate_is_deterministic_across_two_runs` pass. `boot-actors` /
`boot-cross-core` `expected/rtconfig.txt` contain `match` ladders (allowed
by 702) and no `while`/`for`/`async`/`@actor`. Production path also refuses
forbidden constructs (`rtconfig.rs:1535-1540`).

**Evidence:** `cargo test -p wrela-compiler --lib rtconfig` (8/8 ok);
`plans/M11.md:128-135`; `rtconfig.rs:1488-1498`, `:1646-1666`; ledger
`compiler.rtconfig.facts-only` (`ledger/ledger.toml:2120-2127`).

**Owner:** orchestrator re-run.

### L-11 · **medium** · Decision 702 / ledger still say “exactly one `@placed` static”

**Claim:** Decision 702 and `compiler.rtconfig.facts-only` note claim
exactly one `@placed` static of the runtime-tables type
(`plans/M11.md:128-131`; `ledger/ledger.toml:2127`). Live
`boot-actors/expected/rtconfig.txt` has **123** `@placed` lines (RT, SCHED,
GROUPS, ring/mailbox/stub overlays, etc.). Unit test
`placed_uses_rtdata_base_literal` already admits empty-turn RT may *not*
claim `RTDATA_BASE`. Prose is false; generator behavior is intentional
post–E–J.

**Evidence:** `plans/M11.md:128-131`; `ledger/ledger.toml:2120-2127`;
`rg -c '@placed' tests/golden/boot-actors/expected/rtconfig.txt` → 123;
`rtconfig.rs` `placed_uses_rtdata_base_literal`.

**Owner:** **M** — amend 702 / ledger note to “facts + exhaustive match +
N placed overlays,” or freeze **860**.

---

## 4. Decision 810 carve-out

### L-12 · **info** · Exact-name only; primary + secondary covered; ordinary sync rejected

**Claim:** `is_runtime_event_loop_entry` matches only
`__wrela_rt_secondary_entry` | `__wrela_rt_primary_entry`
(`sema/bodies.rs:2248-2254`). Docs 02 §8.1, ledger
`sema.bounds.loops.event-loop-entry` (`test`), and
`golden/check-budget-event-loop-entry` cover both names;
`golden/err-budget-sync-unbounded` still rejects an ordinary sync `while`.
Thin `__wrela_secondary_entry_{1,2}` wrappers have no loops (not on the
allowlist — correct). Carve-out is **fn-wide** (inner suspend `while` in
`__wrela_rt_primary_entry` also omits `@budget` — by design).

**Evidence:** `sema/bodies.rs:2248-2264`; `stdlib/core/runtime.wr:1420-1436`,
`:1664+`; `tests/golden/check-budget-event-loop-entry/input.wr`;
`tests/golden/err-budget-sync-unbounded/expected/check.txt`;
`ledger/ledger.toml:172-176`; decision 854 `plans/M11.md:727-730`.

**Owner:** none (no hole found).

---

## 5. Oracle honesty (E–J vs K)

### L-13 · **info** · E–J commit messages’ byte-identical `test.txt` claims hold

**Claim:** Commits `9584d58` (E), `f9e80bb` (F), `c58e9d6` (G), `4850125`
(H), `fb0b25b` (I), `0fc3435` (J) each claim boot transcripts
byte-identical; each touches **0** `expected/test.txt` files (report /
layout-assert / rtconfig may move).

**Evidence:** `git show --name-only` per sha; commit message bodies.

**Owner:** orchestrator spot-check.

### L-14 · **info** · K disclosed churn; host `test.txt` unchanged

**Claim:** Decision 856 + K commit (`24064ba`) allow transcript/report churn
from generic runner + RTDATA 256 KiB; K touched 54 `report.txt`, img.hex,
rtconfig — **0** `test.txt`. Consistent with “host-visible test line shape
unchanged,” not a silent dual path. Specialized `emit_rt_*` ImageStatic
emitters are gone (comments + deleted bodies); leftover `emit_*_call` /
inject stubs are NON_INVENTORY.

**Evidence:** `24064ba` stat; `plans/M11.md:737-741`;
`emitted_a64_census.rs:166-194`.

**Owner:** none.

### L-15 · **low** · Stale comments still name deleted `build_entry_driver` / `build_rt_*`

**Claim:** VMM / harness comments still describe `build_entry_driver` /
`build_rt_drain` / `build_rt_enqueue` as if live
(`wrela-vmm/src/lib.rs:2215`, `:5114`, `:3216`; harness history comments).
Confusing for auditors; not dual-path code.

**Evidence:** ripgrep hits above.

**Owner:** **post-M11** comment hygiene (or **M** if evidence cites them).

---

## 6. RTDATA_BASE ratchet (64 → 128 → 256 KiB)

### L-16 · **high** · `machine.layout.v1-constants` ledger note stops at 128 KiB

**Claim:** Live `RTDATA_BASE = IMAGE_BASE + 256 KiB` (`0x4054_0000`)
(`wrela-machine/src/lib.rs:125`); docs `06-machine.md:38-43` and
REVIEW-QUEUE packing line record decision **858**. Ledger clause
`machine.layout.v1-constants` note ends at item H / decision **818**
(`0x4052_0000`) with **no 858 / 256 KiB paragraph**
(`ledger/ledger.toml:986`). `MACHINE_REVISION` / `MACHINE_REVISION_STR`
still unbumped (`wrela-machine/src/lib.rs:21`, `:28`) — intentional
(722/752/818/858). Packing still fail-closed (`layout.rs:519-525`,
`RTDATA_SIZE_MAX`).

**Evidence:** files above; `plans/M11.md:749-754`; REVIEW-QUEUE M11 packing
line (~49).

**Owner:** **M** (or fix-now) — append 858 to the ledger note in the close
commit.

### L-17 · **info** · MACHINE_REVISION deliberately unbumped (coherent)

**Claim:** Docs, plan decisions, and REVIEW-QUEUE agree: packing base is not
a guest-visible revision bump.

**Evidence:** `plans/M11.md:222-228`, `:267-269`, `:749-754`;
`wrela-machine/src/lib.rs:123-125`.

**Owner:** none.

---

## 7. `runtime.wr` algorithms / leftover emitters

### L-18 · **high** · Migrated algorithms live in layout reinject, not dump inventory

**Claim:** Same root cause as L-04. `runtime.wr` holds the algorithms
(boot transcripts green; reinject seeds dozens of `__wrela_rt_*` keys in
`layout/harness.rs:1153-1216`). Exit’s dump-stage wording overclaims the
review surface. Specialized ImageStatic emitters named in the dissolve
inventory are deleted; remaining hand emitters are floor + unclassified
micro-helpers + NON_INVENTORY inject stubs.

**Evidence:** `layout/harness.rs:1153-1216`; L-04 dump probes;
`emitted_a64_census.rs:133-232`.

**Owner:** **M** (tie to L-04 exit rewrite).

---

## 8. Dead code / warnings after K

### L-19 · **medium** · Lib-build warnings: `HarnessAddrs` / Asm helpers unused

**Claim:** `cargo build -p wrela-compiler` warns: `HarnessAddrs` never
constructed; `production` unused; Asm methods `addr`, `patch_load_imm`,
`bl_to`, `b_to`, `skip_placeholder`, `patch_cbnz` never used. `ring_base` /
`data_base` already `#[allow(dead_code)]`; `exit_mmio_addr` unread.
`push_abort_tail` is `#[cfg(test)]` only. Residues of deleted
`build_entry_driver` / console hand-asm — incomplete deletion, not a
silent ImageStatic row (census still locks live floor words).

**Evidence:** build warnings 2026-07-26;
`layout/harness.rs:469-497`, `:547+`, `:636-646`, `:751-752`.

**Owner:** **M** or **post-M11** — delete or cfg(test)-gate the dead Asm
surface so “K deleted the driver” is mechanically true.

---

## 9. Dishonest COMPLETE — additional blockers

### L-20 · **blocker** · Item M evidence block / mechanical exit verification not done

**Claim:** Item M still open (`plans/M11.md:912-914`). This file is L only;
no COMPLETE evidence block yet.

**Evidence:** plan checkboxes; `git log -1` → `55da7c3` Merge item K.

**Owner:** **M**.

### L-21 · **medium** · `sema.bounds.loops` remains `gap` (honest) — exit “narrowed” OK

**Claim:** Exit says named clauses flipped **or narrowed**
(`plans/M11.md:938-940`). Sync half narrowed; status stays `gap` for
async/ISR — matches ledger. Not a close blocker if M cites the note, not
a false `test` flip.

**Evidence:** `ledger/ledger.toml:166-169`; `plans/M11.md:938-940`.

**Owner:** **M** evidence wording.

---

## Suggested decisions (860–864 band — orchestrator may skip if prose-only)

| # | Proposal |
|---|----------|
| **860** | Amend decision 702 / `compiler.rtconfig.facts-only` note: facts-only allows **N** `@placed` overlays + exhaustive `match` ladders; “exactly one” is retired. |
| **861** | Exit FLOOR bullet = measured **52** with extraction trail 811/821/852 (including Unclassified→floor trampoline), not “unchanged at 26 / ImageStatic-only.” |
| **862** | Exit dump-stage bullet = algorithms reachable via **layout reinject / image / boot goldens**; dump `--stage=asm` force-root set stays the M10 console/abort set unless deliberately expanded. |
| **863** | (optional) Delete or `#[cfg(test)]`-gate dead `HarnessAddrs` / unused Asm helpers left after K. |
| **864** | (optional) Ledger `machine.layout.v1-constants` note gains decision 858 (256 KiB) in the M commit. |

---

## Counts

| Severity | Count |
|----------|------:|
| blocker  | 5 |
| high     | 3 |
| medium   | 5 |
| low      | 1 |
| info     | 7 |
| **Total** | **21** |

---

## Gate status (partial)

- `cargo test -p wrela-compiler --lib emitted_a64` — **green** (3).
- `cargo test -p wrela-compiler --lib rtconfig` — **green** (8).
- Focused `wrela dump` probes — **as cited** (boot-actors stages ok but
  omit migrated rt_*; bare `runtime.wr` panics).
- Full `cargo xtask check` — **not run** in this sweep (orchestrator
  should run before M).

## Top blockers for M (short list)

1. **L-02** — FLOOR exit prose vs FLOOR_WORDS=52 / Unclassified extraction.
2. **L-04 / L-18** — dump-stage reachability overclaim.
3. **L-05** — missing post-M `bench guest` delta number.
4. **L-07** — M12 draft presence vs “does not exist” (main tree).
5. **L-16** — ledger note lag on RTDATA 256 KiB (high; pairs with close hygiene).
