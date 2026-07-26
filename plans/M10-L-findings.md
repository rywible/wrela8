# M10 item L — adversarial sweep findings

Worktree: `/Users/ryanwible/projects/wrela8-m10-l`, branch `m10-item-l`, base `14d3c06` (post Waves 0–6 + K).

Orchestrator: verify each claim independently. Severity: **blocker** (item M cannot honestly close), **high**, **medium**, **low**, **info**.

---

## 1. Exit criteria vs F0 census (mechanically false as written)

### L-01 · **blocker** · Exit criterion “only three emitters” is false and untested as stated

**Claim:** Exit criteria require that the only functions in `crates/wrela-compiler/` emitting instruction words are the entry stub, checkpoint/vector stub, and halt tail (`plans/M10.md:1910-1917`). F0 documents the opposite: 24 locked census rows, 714 words of ImageStatic specialization, 94-word `build_entry_driver` residue, etc. (`emitted_a64_census.rs:132-303`, item F0 table `plans/M10.md:1813-1817`).

**Evidence:** `plans/M10.md:1910-1917`; `crates/wrela-compiler/src/emitted_a64_census.rs:14-20`, `:132-303`; F0 done note `plans/M10.md:1813-1817`.

**Owner:** **M** — rewrite exit criteria to match decisions 613/620/670/685 (ImageStatic + stated I residue + F0 ratchet), or finish migrations before claiming “only three functions.”

### L-02 · **blocker** · Exit criterion “measured floor today is 20” contradicts F0 (`FLOOR_WORDS` = 26)

**Claim:** Exit text says combined floor is ≤30 with “measured floor today is 20 (5+5+8+2)” (`plans/M10.md:1913-1914`). F0 locks **26** unique floor words (`push_halt` 15 + SP-only 5 + abort tail 6, overlap removed) (`emitted_a64_census.rs:307-318`, test `emitted_a64_census_matches_live_measurements` `:713-718`).

**Evidence:** `plans/M10.md:1913-1914`; `emitted_a64_census.rs:318`, `:713-718`.

**Owner:** **M** — fix exit prose (26, and category breakdown must include cat4 abort tail + embedded cat2 in checkpoint if ever extracted).

### L-03 · **blocker** · Exit criterion names checkpoint/vector as floor-only; census classifies it as ImageStatic

**Claim:** Exit criteria list “checkpoint/vector stub” among the three sole emitters (`plans/M10.md:1911-1912`). Post–item G, production checkpoint bytes come from `emit_checkpoint_and_vector_stub` (ImageStatic, 26 words REF) (`emitted_a64_census.rs:234-240`); item G explicitly kept 5 floor-cat2 words *inside* that body (`plans/M10.md:1845-1856`).

**Evidence:** `plans/M10.md:1911-1912`, `:1845-1856`; `emitted_a64_census.rs:234-240`.

**Owner:** **M** — align exit criteria with decision 670/673 (floor words embedded in ImageStatic emitters until extraction).

### L-04 · **blocker** · `build_rt_select_and_run` / `build_rt_enqueue` still exist; exit says they must not

**Claim:** Exit criteria require `build_rt_select_and_run`, `build_rt_enqueue`, … **do not exist** (`plans/M10.md:1919-1922`). Both remain public in `layout/harness.rs` (`build_rt_enqueue` `:537`, `build_rt_select_and_run` `:586`), re-exported from `layout.rs:162`, used by VMM tests (`wrela-vmm/src/lib.rs:3207+`) and JIT harness (`harness.rs:3049+`). Plan item F2 documents them as “JIT materialize only” (decision 637) — intentional tension with exit text, not with code.

**Evidence:** `plans/M10.md:1919-1922`, `:1839`; `layout/harness.rs:537`, `:586`; `layout.rs:162`; `wrela-vmm/src/lib.rs:3207`.

**Owner:** **M** — amend exit criteria (JIT/HVF materializers allowed under NON_INVENTORY) or delete/rename symbols so the literal “do not exist” check can pass.

### L-05 · **blocker** · “Floor words pinned as a byte golden” — not satisfied for the full floor

**Claim:** Exit criteria require the floor’s words as a **byte golden** (`plans/M10.md:1918`). Only partial coverage exists: unit test `install_abort_tail_floor_replaces_the_stub_with_the_long_jump` decodes six abort-tail words (`layout/harness.rs:2879-2912`), not `push_halt` (15), entry-stub SP prefix (5), or the combined 26-word floor. No `tests/golden/*` pins floor bytes.

**Evidence:** `plans/M10.md:1918`; `layout/harness.rs:2879-2912`; glob search for floor byte goldens under `tests/golden/` (none).

**Owner:** **M** (or **post-M10** if exit criteria are rewritten to “F0 ratchet + abort-tail inverse test”).

### L-06 · **blocker** · F0 test ratchets inventory, not the exit criterion’s “≤30 / three functions only”

**Claim:** Exit criteria say a test asserts the floor “in the shape `sema::intrinsics`'s surface test” (`plans/M10.md:1915-1917`). F0 tests lock per-emitter counts, `FLOOR_WORDS == 26`, and closed `encode::enc_` sets (`emitted_a64_census.rs:649-771`) but **do not** fail if ImageStatic or `build_entry_driver` grow — by design (`emitted_a64_census.rs:16-20`).

**Evidence:** `plans/M10.md:1915-1917`; `emitted_a64_census.rs:16-20`, `:649-771`.

**Owner:** **M** — either narrow exit criteria to what F0 proves, or add a second test that enforces the ROADMAP-shaped floor (likely impossible without finishing migrations).

### L-07 · **blocker** · `runtime.*` ledger clauses: exit requires `test`, ledger has **zero** `runtime.*` ids

**Claim:** Exit criteria: “`runtime.*` … clauses are `test`, not `gap`” (`plans/M10.md:1929-1930`). Decision 564 split layout rules to `hardware.layout.*`; **`grep '^id = "runtime\.' ledger/ledger.toml` is empty**. Nothing satisfies a mechanical `runtime.*` status check as written.

**Evidence:** `plans/M10.md:1929-1930`, `:495-501`; `ledger/ledger.toml` (no `runtime.*` clause ids).

**Owner:** **M** — add `runtime.*` clauses for migrated scheduler/mailbox source, or change exit criteria to cite concrete ids (e.g. `actors.scheduling.*`, stdlib-loaded paths).

### L-08 · **high** · Exit requires M10 `bench guest` before/after delta as a number — not recorded

**Claim:** Exit criteria require `bench guest` locked on `boot-actors` **and** “this plan records the before/after delta as a number” (`plans/M10.md:1934-1935`). Decision 661 records lock medians (~67ms, threshold 700ms) (`plans/M10.md:971-972`, `:1556-1557`) but **no pre-M10 vs post-M10 scheduler migration delta** appears in `plans/M10.md` or `bench/thresholds.toml`.

**Evidence:** `plans/M10.md:1934-1935`, `:971-972`; `bench/thresholds.toml:108-138`.

**Owner:** **M** — capture baseline from base commit or disclose “delta N/A (byte-identical transcripts only)” in evidence block.

### L-09 · **high** · Per-stage goldens “per migrated routine” — mostly asm-only, no typed/flowwir/mwir runtime suite

**Claim:** Exit criteria require runtime wrela source reachable via `wrela dump --stage={typed,flowwir,mwir,asm}` with **at least one golden per stage per migrated routine** (`plans/M10.md:1924-1926`). `stdlib/core/runtime.wr` exists, but dedicated `typed-*` / `flowwir-*` / `mwir-*` goldens naming `rt_enqueue`, `rt_run_one`, `__wrela_line_*`, etc. are absent; coverage is indirect via fat `check-*-backend/expected/asm.txt` blobs.

**Evidence:** `plans/M10.md:1924-1926`; `stdlib/core/runtime.wr`; `tests/golden/flowwir-*` (5 cases, no runtime module dumps); asm mentions in `tests/golden/check-list-backend/expected/asm.txt` (etc.).

**Owner:** **M** or **post-M10** (depends on whether “per routine” means each glue key or each migration item).

---

## 2. `emitted_a64_census.rs` vs reality

### L-10 · **medium** · F0 summary table in `plans/M10.md` stale vs live census after G/H/I

**Claim:** Item F0 done table still lists not-yet **587**, image-static **300**, unclassified **23**, grand **951** (`plans/M10.md:1805-1811`). Live constants: not-yet **7**, image-static **714**, unclassified **117**, grand **879** (`emitted_a64_census.rs:320-329`).

**Evidence:** `plans/M10.md:1805-1811`; `emitted_a64_census.rs:320-329`.

**Owner:** **fix now** (doc-only) or **M** evidence block.

### L-11 · **medium** · `layout/harness.rs::push_turn_addr_from_id` is dead production code, still NotYetMigrated (7 words)

**Claim:** Hand-asm `push_turn_addr_from_id` in harness is **only** called from `emitted_a64_census_live_counts` (`harness.rs:2589-2590`). Production uses `codegen::push_turn_addr_from_id` (`codegen.rs:2835+`, listed in `BACKEND_EMITTERS` `:616`). Harness copy is orphan dead code still counted as not-yet migrated.

**Evidence:** `layout/harness.rs:482-490`, `:2589-2590`; `emitted_a64_census.rs:264-270`, `:616`; `layout.rs:1426`.

**Owner:** **fix now** (delete harness copy + census row) or **post-M10** cleanup.

### L-12 · **low** · Census comment still equates deleted builders with live JIT names

**Claim:** `GRAND_TOTAL_SUM_OF_ROWS` comment references `build_rt_enqueue` == `build_ring_enqueue` (`emitted_a64_census.rs:327-328`); `build_ring_enqueue` is gone. Misleading for reviewers, not a test failure.

**Evidence:** `emitted_a64_census.rs:327-328`.

**Owner:** **fix now** (comment).

### L-13 · **info** · F0 live tests green at base (sanity)

**Claim:** `cargo test -p wrela-compiler emitted_a64` passes (3 tests: live counts, closed set, enc site scan).

**Evidence:** run 2026-07-26 on `m10-item-l`.

**Owner:** orchestrator re-run.

---

## 3. Hand-asm / `encode::enc_` outside declared floor + ImageStatic

### L-14 · **high** · Production images still enter via hand-emitted `build_entry_driver` (94 words, unclassified)

**Claim:** Item I closed as **stated residue** (685–689), not wrela migration. `layout_test_image` / `layout_program` still call `build_entry_driver` (`harness.rs:1896`, `:1160+`). Census: unclassified **117** words including **94** for that symbol (`emitted_a64_census.rs:296-302`, `:324`).

**Evidence:** `plans/M10.md:1871-1887`; `layout/harness.rs:1160`, `:1896`; `emitted_a64_census.rs:296-302`.

**Owner:** **post-M10** (accepted residue) but **M** must not claim “no hand-emitted outside floor.”

### L-15 · **medium** · `build_checkpoint_and_vector_stub_ex` remains on production layout paths (wrapper, not census row)

**Claim:** `layout.rs` and `layout_test_image` still build checkpoint blocks via `build_checkpoint_and_vector_stub_ex` (`layout.rs:2459`, `:2641`; `harness.rs:1860`), which forwards to `emit_checkpoint_and_vector_stub`. Listed in `NON_INVENTORY` (`emitted_a64_census.rs:633-635`) — correct — but any new `enc_` in the wrapper bypasses per-function word lock if not registered.

**Evidence:** `layout.rs:2459`; `harness.rs:145-182`, `:1860`; `emitted_a64_census.rs:633-635`.

**Owner:** **info** for orchestrator (verify wrapper stays thin); **M** if wrapper grows.

### L-16 · **low** · Stale line references in plan “Raw addresses” / ROADMAP inventory (pre-K paths)

**Claim:** `plans/M10.md` still cites `layout.rs:432-596` for checkpoint stub and `layout.rs:6012+` for cross-core paths (`plans/M10.md:119-121`, `:127-130`). Post-K, checkpoint/JIT materializers live in `layout/harness.rs`; grep shows logic moved (orchestrator should re-verify line numbers before citing in M evidence).

**Evidence:** `plans/M10.md:119-130`; `layout/harness.rs:139+`; item K `plans/M10.md:1889-1898`.

**Owner:** **M** evidence block / doc hygiene.

---

## 4. Ledger gaps that should have flipped

### L-17 · **medium** · `hardware.layout.runtime-class` note still says `placed-*` are `gap`; clauses are `test`

**Claim:** Clause `hardware.layout.runtime-class` has `status = "test"` (`ledger/ledger.toml:1979-1981`), but its note tail still states `hardware.layout.placed-static` / `placed-verified` “both still `gap`” (`ledger/ledger.toml:2082`). Those two clauses are `status = "test"` at `:2085-2104`.

**Evidence:** `ledger/ledger.toml:2082`, `:2085-2104`.

**Owner:** **fix now** (ledger note trim) — **M** if flip narrative required.

### L-18 · **medium** · `machine.bench.guest-lane-locked` cites wrong workload in `tests` + note

**Claim:** Decision 661 moved locked guest lane to `boot-actors` (`xtask` `bench_guest_lane`, `bench/thresholds.toml:boot_actors_median_us`). Ledger clause still lists `tests = ["xtask:bench", "golden/boot-hello"]` and note describes `boot_hello_median_us` (`ledger/ledger.toml:572-576`). Mechanical ledger validation may still pass (xtask:bench runs boot-actors), but **cited golden is wrong** for auditors.

**Evidence:** `ledger/ledger.toml:572-576`; `crates/xtask/src/main.rs:7610+`; `bench/thresholds.toml:108-138`; `plans/M10.md:949-953`.

**Owner:** **fix now** (ledger tests + note).

### L-19 · **info** · `hardware.layout.{runtime-class,placed-static,placed-verified}` are already `test`

**Claim:** Exit subset for hardware layout is satisfied at status level; issue is stale notes (L-17) and missing `runtime.*` (L-07).

**Evidence:** `ledger/ledger.toml:1979-2104`.

**Owner:** **M** close checklist.

---

## 5. Dead comments / known risks vs item G

### L-20 · **medium** · Known risks still assign `group_service_ctx` driver-turn fix to item G (G done)

**Claim:** “Known risks” say item **G owns** messageable-driver turn omission (`plans/M10.md:1961-1968`). Item G done note: decision **671**, `group_service_ctx_includes_messageable_driver_turns`, golden `boot-deadline-driver-turn` (`plans/M10.md:1856-1858`, `layout.rs:6740-6793`).

**Evidence:** `plans/M10.md:1961-1968`, `:1856-1858`; `layout.rs:6746-6793`.

**Owner:** **fix now** (strike or retarget risk paragraph).

---

## 6. Transcript / golden / census drift from K split (`layout/harness.rs`)

### L-21 · **low** · Golden comments and ledger notes still say `layout::build_rt_enqueue` / `build_entry_driver`

**Claim:** K moved emitters to `layout/harness.rs` with re-exports (`plans/M10.md:1895-1897`, decision 690). Comments in goldens (`tests/golden/err-boot-dma-region-handoff-group/...`, `boot-receipt-handoff/...`) and long ledger notes (`machine.boot.no-discovery` `:546`) still describe `layout::build_*` paths — confusing for transcript archaeology, not failing tests.

**Evidence:** `tests/golden/err-boot-dma-region-handoff-group/src/examples/err_boot_dma_region_handoff_group.wr:21`; `ledger/ledger.toml:546` (build_entry_driver hook).

**Owner:** **post-M10** comment hygiene.

### L-22 · **info** · K census key paths updated; enc site split locked

**Claim:** Item K documents `ENCODE_ENC_SITES_BY_FILE` = codegen 811 / layout 8 / harness 68 (`plans/M10.md:1897-1898`; `emitted_a64_census.rs:338-342`). F0 enc-site test passed at base.

**Evidence:** `plans/M10.md:1897-1898`; `emitted_a64_census.rs:338-342`.

**Owner:** orchestrator re-run after any harness edit.

---

## 7. Item M close — additional blockers

### L-23 · **blocker** · No M10 evidence block / item L–M checklist commit on branch yet

**Claim:** Item M requires exit criteria verified mechanically + evidence block (`plans/M10.md:1904-1905`). Branch `m10-item-l` at `14d3c06` is item K only; this file is the L deliverable.

**Evidence:** `git log -1 --oneline` → `14d3c06 M10 item K…`.

**Owner:** **M**.

### L-24 · **medium** · Item H done note uses inconsistent census baseline (232→222 vs G’s 128→7)

**Claim:** Item H claims “Census: not-yet 232→222” (`plans/M10.md:1868-1869`) but item G already reported not-yet **128→7** (`plans/M10.md:1859-1860`). Arithmetic in plan history is internally inconsistent (orchestrator: re-derive from git).

**Evidence:** `plans/M10.md:1859-1860`, `:1868-1869`.

**Owner:** **M** evidence block only.

### L-25 · **low** · `plans/M11.md` absent — satisfies one exit criterion

**Claim:** Exit criteria require `plans/M11.md` does not exist yet (`plans/M10.md:1936`) — true at sweep time.

**Evidence:** glob `plans/M11.md` → none.

**Owner:** **M** (keep absent until close).

---

## Suggested decisions (695–697 band — orchestrator may renumber)

| # | Proposal |
|---|----------|
| **695** | Exit criteria at `plans/M10.md:1907-1936` are **not** closable literally; item M must ship amended criteria matching F0 + decisions 613/620/670/685/690 (ImageStatic + stated residues + JIT NON_INVENTORY), or defer “M10 closed” until migrations finish. |
| **696** | “Floor byte golden” means **full 26-word floor** hex pin **or** explicit downgrade to F0 ratchet + abort-tail inverse test (`harness.rs:2879-2912`) — pick one in M commit. |
| **697** | `runtime.*` exit bullet means **new ledger clause ids** for migrated runtime source, not `hardware.layout.*` — add clauses or rewrite bullet before `xtask ledger` gate for M. |

---

## Counts

| Severity | Count |
|----------|------:|
| blocker  | 8 |
| high     | 4 |
| medium   | 8 |
| low      | 4 |
| info     | 3 |
| **Total** | **27** |

---

## Gate status (partial)

- `cargo test -p wrela-compiler emitted_a64` — **green** (2026-07-26).
- Full `cargo xtask check` — **not run** in this sweep (orchestrator should run before M).
