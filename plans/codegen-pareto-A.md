# codegen-Pareto item A — Lane 2 derived tables

Findings file for item A (decision 1709: per-item findings, so parallel
items do not collide on the plan's prose).
Code: `crates/wrela-compiler/src/cost/derive.rs`.
Decisions **1720–1726**. Commit: `cp-A`.

Item A turns the one measured artifact M20 item C committed —
`tests/golden/boot-actors/lane2-freq.txt`, a `<fn_key>#<block_index> =
<count>` vector taken from the host DRAM snapshot of a real boot — into the
three tables the plan keys off. **Nothing in this item is on the compile
path.** No emission, layout, or report path was touched; `derive.rs` is a
new module plus its re-exports, and no existing caller reaches it yet.

The tables below are the committed artifact. They are spliced from
`DerivedTables::render()` rather than retyped, and regenerate byte-identically
with:

```
cargo test -p wrela-compiler --lib cost::derive::tests::print_the_committed_tables -- --nocapture
```

---

## 1. The vector, and a correction to M20's evidence block

```
workload=boot-actors sidecar_digest=0x4a5361690b06f87a
hits total=6647 artifact=3938 measured=2709
fns=67 keys=364 loops=23
```

**The sidecar holds 372 keys, not 376.** `plans/M20.md` line 39 (the
evidence block) says:

> Committed sidecar `tests/golden/boot-actors/lane2-freq.txt` carries **376**
> keys.

That number is wrong. The file holds **372** `key=count` lines; its own
generated header says 372 ("carried only 128 of these 372 pair(s)"); and
M20 contradicts itself twice elsewhere, at line 842 ("actual non-zero hit
blocks are 372") and line 911 ("372 measured non-zero hit blocks"). Only
the evidence block carries 376. Item A did not edit `plans/M20.md` — a
milestone's evidence block is not this item's to rewrite — but the number
should be corrected there by whoever next touches it.

Total hit mass across all 372 keys is **6647**.

## 2. `__wrela_rt_primary_boot` is an instrumentation artifact (decision 1724)

The single load-bearing finding of this item. `__wrela_rt_primary_boot`
contains the loop that zeroes the Lane 2 counter pool:

```
@budget(bound=4096)
while zj < BLOCK_POOL_COUNT:
    LANE2.hits[zj] = 0
```

Its own blocks increment their counters *while that same loop walks
`LANE2.hits[0..3072]` clearing them*, so each block ends holding **the
number of iterations remaining after its own global id was passed** — a
function of where codegen happened to place that block in the id space, not
of the workload. Measured on `boot-actors` those four blocks hold
986/984/983/981. Its entry block `#0` is absent from the sidecar entirely,
because the zeroing loop wiped it after it ran.

That fn is **3938 of the vector's 6647 hits — 59.2%**. Left in, the
counter-clearing loop is the hottest code in the image and every derived
table is a table about instrumentation.

It is excluded **by name** (`derive::COUNTER_CLEARING_KEY`), the same shape
`codegen::BLOCK_HIT_KEY` already uses for the counter helper, and the removed
mass is **reported, not hidden**: `DerivedTables::artifact_hits` carries it,
and `render()` prints `total=6647 artifact=3938 measured=2709` on its first
line. A sidecar that is *nothing but* the artifact is an error, not an empty
table (`unit:an_artifact_only_sidecar_is_not_a_measurement`).

After exclusion: **67 fns, 364 keys, 2709 measured hits, 23 loop runs**,
sidecar digest `0x4a5361690b06f87a`.

---

## 3. Table 1 — per-loop measured trip counts

Grain is a **contiguous run of loop-resident block indices** (decision
1722), not a proved natural loop: a gap splits one source loop into two
rows, so the row count is a lower bound on how many loops there are and an
exact statement about the blocks each row names.

| run | calls | blocks | peak f | trips | hits |
| --- | --- | --- | --- | --- | --- |
| `copy_bytes_range#1..3` | 1 | 3 | 13 | 13.000 | 37 |
| `copy_bytes_range#5..5` | 1 | 1 | 12 | 12.000 | 12 |
| `__wrela_lane1_dump#5..8` | 1 | 4 | 7 | 7.000 | 19 |
| `__wrela_rt_primary_entry#10..11` | 2 | 2 | 13 | 6.500 | 25 |
| `__wrela_rt_primary_entry#13..13` | 2 | 1 | 12 | 6.000 | 12 |
| `__wrela_rt_primary_entry#22..22` | 2 | 1 | 12 | 6.000 | 12 |
| `__wrela_rt_primary_entry#15..15` | 2 | 1 | 11 | 5.500 | 11 |
| `__wrela_lane1_dump#10..17` | 1 | 8 | 5 | 5.000 | 36 |
| `__wrela_rt_boot_init#5..8` | 1 | 4 | 4 | 4.000 | 10 |
| `copy_line_buf_range#1..3` | 33 | 3 | 122 | 3.696 | 300 |
| `__wrela_lane1_dump#1..3` | 1 | 3 | 3 | 3.000 | 7 |
| `__wrela_rt_boot_init#1..3` | 1 | 3 | 3 | 3.000 | 7 |
| `copy_line_buf_range#5..5` | 33 | 1 | 89 | 2.696 | 89 |
| `__wrela_lane1_sum_method_hits#1..1` | 5 | 1 | 10 | 2.000 | 10 |
| `__wrela_rt_boot_init#10..11` | 1 | 2 | 2 | 2.000 | 4 |
| `__wrela_lane1_sum_messages#1..1` | 1 | 1 | 2 | 2.000 | 2 |
| `__wrela_lane1_sum_run_one#1..1` | 1 | 1 | 2 | 2.000 | 2 |
| `__wrela_lane1_sum_turns#1..1` | 1 | 1 | 2 | 2.000 | 2 |
| `__wrela_fmt_dec#10..10` | 15 | 1 | 27 | 1.800 | 27 |
| `__wrela_rt_run_one#13..16` | 11 | 4 | 18 | 1.636 | 68 |
| `__wrela_rt_run_one#9..11` | 11 | 3 | 18 | 1.636 | 54 |
| `__wrela_rt_run_one#19..20` | 11 | 2 | 18 | 1.636 | 32 |
| `__wrela_lane1_method_flat#1..1` | 16 | 1 | 26 | 1.625 | 26 |

### The unrolling verdict

**Do not pull unrolling in from the backlog on this evidence.**

The peak measured trip count in the entire workload is **13.000**, and that
loop (`copy_bytes_range#1..3`) runs **once** in the whole boot — 37 hits
total. The busiest loop by hit mass is `copy_line_buf_range#1..3` at
**3.696 trips** over 33 calls, and that is exactly the ladder's
"copy-loop unroll" candidate: a 3.7-trip loop is below any unroll factor
worth the words. Nothing measured here clears the bar.

Pinned by `unit:the_peak_measured_trip_count_is_thirteen`, which asserts
both the peak (13.000, 1 call) and the busiest (3.696, 300 hits) so the
verdict fails loudly if a future workload moves either.

---

## 4. Table 2 — per-fn call frequency (all 67 rows)

`calls` is `f(fn#0)` (decision 1721). Leader 0's span starts at word 0 of
the fn, so every **call** runs it. For an async fn that counts *fresh
entries only* — the dispatch header branches straight to
`state_flat_base[k]` on a resume, past block 0. A `-` means block 0 was
absent (the artifact fn is the only measured instance, and it is excluded).

**F6, the consumer this table was built for, was cut at activation
(decision 1770). Nothing is built on it here.** It is committed because it
is the cross-oracle for decision 1721 and because it is cheap to keep.

| fn | calls | hot blocks | hits |
| --- | --- | --- | --- |
| `copy_line_buf_range` | 33 | 6 | 455 |
| `__wrela_console_append_line_buf` | 33 | 3 | 99 |
| `__wrela_mb_get_count` | 24 | 4 | 57 |
| `__wrela_mb_method_count` | 23 | 4 | 54 |
| `__wrela_mb_load_word` | 18 | 4 | 42 |
| `__wrela_mb_store_word` | 18 | 4 | 42 |
| `__wrela_lane1_method_flat` | 16 | 8 | 108 |
| `__wrela_fmt_dec` | 15 | 15 | 172 |
| `__wrela_rt_select` | 14 | 28 | 199 |
| `ascii_digit` | 14 | 13 | 66 |
| `__wrela_select_root` | 14 | 5 | 49 |
| `__wrela_mb_turn_index` | 14 | 4 | 35 |
| `__wrela_mb_capacity` | 14 | 4 | 34 |
| `__wrela_mb_set_count` | 14 | 4 | 34 |
| `__wrela_mb_slot_words` | 14 | 4 | 34 |
| `__wrela_try_select` | 14 | 2 | 28 |
| `__wrela_rt_checkpoint` | 13 | 8 | 104 |
| `__wrela_lane1_append_u64` | 13 | 1 | 13 |
| `__wrela_test_turn_index` | 12 | 2 | 24 |
| `__wrela_rt_run_one` | 11 | 20 | 259 |
| `__wrela_call_method` | 11 | 12 | 47 |
| `__wrela_method_is_aggregate` | 11 | 12 | 47 |
| `__wrela_method_suspends` | 11 | 12 | 47 |
| `__wrela_lane1_record_method` | 11 | 3 | 33 |
| `__wrela_drain_reply_count` | 11 | 2 | 22 |
| `__wrela_drain_request_count` | 11 | 2 | 22 |
| `__wrela_select_count` | 11 | 2 | 22 |
| `__wrela_try_drain` | 11 | 2 | 22 |
| `__wrela_rt_enqueue` | 7 | 9 | 60 |
| `__wrela_deliver_reply` | 7 | 4 | 19 |
| `__wrela_mb_core` | 7 | 4 | 17 |
| `__wrela_mb_get_head` | 7 | 4 | 17 |
| `__wrela_mb_get_tail` | 7 | 4 | 17 |
| `__wrela_mb_has_lineage` | 7 | 4 | 17 |
| `__wrela_mb_set_head` | 7 | 4 | 17 |
| `__wrela_mb_set_tail` | 7 | 4 | 17 |
| `__wrela_enqueue_local` | 7 | 1 | 7 |
| `__wrela_lane1_sum_method_hits` | 5 | 6 | 35 |
| `__wrela_line_begin` | 5 | 1 | 5 |
| `__wrela_line_commit` | 4 | 2 | 8 |
| `__enqueue_0` | 4 | 1 | 4 |
| `Ledger.mark` | 3 | 1 | 3 |
| `__enqueue_1` | 3 | 1 | 3 |
| `__wrela_rt_primary_entry` | 2 | 13 | 71 |
| `__wrela_init_nwords` | 2 | 4 | 5 |
| `__wrela_init_store_word` | 2 | 4 | 5 |
| `__wrela_test_call` | 2 | 2 | 4 |
| `__wrela_lane1_dump` | 1 | 17 | 64 |
| `copy_bytes_range` | 1 | 6 | 51 |
| `__wrela_rt_boot_init` | 1 | 13 | 25 |
| `turns` | 1 | 11 | 11 |
| `Worker.slow` | 1 | 9 | 9 |
| `__wrela_lane1_sum_messages` | 1 | 6 | 7 |
| `__wrela_lane1_sum_run_one` | 1 | 6 | 7 |
| `__wrela_lane1_sum_turns` | 1 | 6 | 7 |
| `Worker.quick` | 1 | 5 | 5 |
| `Worker.report` | 1 | 4 | 4 |
| `__wrela_console_append_bytes` | 1 | 4 | 4 |
| `__wrela_lane2_dump` | 1 | 2 | 2 |
| `__wrela_quiesce_before_halt` | 1 | 2 | 2 |
| `__wrela_rt_summary_and_halt` | 1 | 2 | 2 |
| `__wrela_test_append_prefix` | 1 | 2 | 2 |
| `__wrela_test_suspends` | 1 | 2 | 2 |
| `Ledger.read_marks` | 1 | 1 | 1 |
| `__wrela_append_failed_tail_literal` | 1 | 1 | 1 |
| `__wrela_append_ok_literal` | 1 | 1 | 1 |
| `__wrela_append_passed_comma_literal` | 1 | 1 | 1 |

---

## 5. Table 3 — per-block hot/cold (all 364 measured rows)

Classification is **three-valued** (decision 1723), and the table carries
only the `hot` rows because the other two classes are answered without a
row:

- **Hot** — the sidecar names the fn, count ≥ 1. Every row below.
- **Cold** — the sidecar names the fn, this block is absent. That is a
  *real measured zero*: the generator writes every non-zero counter from a
  pool (`BLOCK_POOL_COUNT` = 3072) that covers all 2527 assigned ids, so
  absence inside a named fn means the counter stayed 0.
- **Unmeasured** — the sidecar never names the fn. No evidence at all.

| block | f | class |
| --- | --- | --- |
| `Ledger.mark#0` | 3 | hot |
| `Ledger.read_marks#0` | 1 | hot |
| `Worker.quick#0` | 1 | hot |
| `Worker.quick#1` | 1 | hot |
| `Worker.quick#2` | 1 | hot |
| `Worker.quick#3` | 1 | hot |
| `Worker.quick#7` | 1 | hot |
| `Worker.report#0` | 1 | hot |
| `Worker.report#1` | 1 | hot |
| `Worker.report#2` | 1 | hot |
| `Worker.report#3` | 1 | hot |
| `Worker.slow#0` | 1 | hot |
| `Worker.slow#1` | 1 | hot |
| `Worker.slow#2` | 1 | hot |
| `Worker.slow#3` | 1 | hot |
| `Worker.slow#7` | 1 | hot |
| `Worker.slow#8` | 1 | hot |
| `Worker.slow#9` | 1 | hot |
| `Worker.slow#10` | 1 | hot |
| `Worker.slow#14` | 1 | hot |
| `__enqueue_0#0` | 4 | hot |
| `__enqueue_1#0` | 3 | hot |
| `__wrela_append_failed_tail_literal#0` | 1 | hot |
| `__wrela_append_ok_literal#0` | 1 | hot |
| `__wrela_append_passed_comma_literal#0` | 1 | hot |
| `__wrela_call_method#0` | 11 | hot |
| `__wrela_call_method#1` | 4 | hot |
| `__wrela_call_method#2` | 3 | hot |
| `__wrela_call_method#4` | 1 | hot |
| `__wrela_call_method#5` | 1 | hot |
| `__wrela_call_method#12` | 7 | hot |
| `__wrela_call_method#13` | 7 | hot |
| `__wrela_call_method#14` | 3 | hot |
| `__wrela_call_method#16` | 4 | hot |
| `__wrela_call_method#17` | 2 | hot |
| `__wrela_call_method#19` | 2 | hot |
| `__wrela_call_method#20` | 2 | hot |
| `__wrela_console_append_bytes#0` | 1 | hot |
| `__wrela_console_append_bytes#3` | 1 | hot |
| `__wrela_console_append_bytes#6` | 1 | hot |
| `__wrela_console_append_bytes#9` | 1 | hot |
| `__wrela_console_append_line_buf#0` | 33 | hot |
| `__wrela_console_append_line_buf#3` | 33 | hot |
| `__wrela_console_append_line_buf#6` | 33 | hot |
| `__wrela_deliver_reply#0` | 7 | hot |
| `__wrela_deliver_reply#1` | 2 | hot |
| `__wrela_deliver_reply#3` | 5 | hot |
| `__wrela_deliver_reply#6` | 5 | hot |
| `__wrela_drain_reply_count#0` | 11 | hot |
| `__wrela_drain_reply_count#1` | 11 | hot |
| `__wrela_drain_request_count#0` | 11 | hot |
| `__wrela_drain_request_count#1` | 11 | hot |
| `__wrela_enqueue_local#0` | 7 | hot |
| `__wrela_fmt_dec#0` | 15 | hot |
| `__wrela_fmt_dec#4` | 15 | hot |
| `__wrela_fmt_dec#6` | 15 | hot |
| `__wrela_fmt_dec#7` | 2 | hot |
| `__wrela_fmt_dec#9` | 13 | hot |
| `__wrela_fmt_dec#10` | 27 | hot |
| `__wrela_fmt_dec#11` | 14 | hot |
| `__wrela_fmt_dec#12` | 14 | hot |
| `__wrela_fmt_dec#14` | 14 | hot |
| `__wrela_fmt_dec#15` | 13 | hot |
| `__wrela_fmt_dec#16` | 14 | hot |
| `__wrela_fmt_dec#17` | 1 | hot |
| `__wrela_fmt_dec#18` | 1 | hot |
| `__wrela_fmt_dec#20` | 1 | hot |
| `__wrela_fmt_dec#21` | 13 | hot |
| `__wrela_init_nwords#0` | 2 | hot |
| `__wrela_init_nwords#1` | 1 | hot |
| `__wrela_init_nwords#3` | 1 | hot |
| `__wrela_init_nwords#4` | 1 | hot |
| `__wrela_init_store_word#0` | 2 | hot |
| `__wrela_init_store_word#1` | 1 | hot |
| `__wrela_init_store_word#3` | 1 | hot |
| `__wrela_init_store_word#4` | 1 | hot |
| `__wrela_lane1_append_u64#0` | 13 | hot |
| `__wrela_lane1_dump#0` | 1 | hot |
| `__wrela_lane1_dump#1` | 3 | hot |
| `__wrela_lane1_dump#2` | 2 | hot |
| `__wrela_lane1_dump#3` | 2 | hot |
| `__wrela_lane1_dump#5` | 2 | hot |
| `__wrela_lane1_dump#6` | 7 | hot |
| `__wrela_lane1_dump#7` | 5 | hot |
| `__wrela_lane1_dump#8` | 5 | hot |
| `__wrela_lane1_dump#10` | 5 | hot |
| `__wrela_lane1_dump#11` | 5 | hot |
| `__wrela_lane1_dump#12` | 5 | hot |
| `__wrela_lane1_dump#13` | 4 | hot |
| `__wrela_lane1_dump#14` | 5 | hot |
| `__wrela_lane1_dump#15` | 5 | hot |
| `__wrela_lane1_dump#16` | 5 | hot |
| `__wrela_lane1_dump#17` | 2 | hot |
| `__wrela_lane1_dump#18` | 1 | hot |
| `__wrela_lane1_method_flat#0` | 16 | hot |
| `__wrela_lane1_method_flat#1` | 26 | hot |
| `__wrela_lane1_method_flat#2` | 10 | hot |
| `__wrela_lane1_method_flat#3` | 10 | hot |
| `__wrela_lane1_method_flat#5` | 10 | hot |
| `__wrela_lane1_method_flat#6` | 10 | hot |
| `__wrela_lane1_method_flat#7` | 10 | hot |
| `__wrela_lane1_method_flat#8` | 16 | hot |
| `__wrela_lane1_record_method#0` | 11 | hot |
| `__wrela_lane1_record_method#3` | 11 | hot |
| `__wrela_lane1_record_method#6` | 11 | hot |
| `__wrela_lane1_sum_messages#0` | 1 | hot |
| `__wrela_lane1_sum_messages#1` | 2 | hot |
| `__wrela_lane1_sum_messages#2` | 1 | hot |
| `__wrela_lane1_sum_messages#3` | 1 | hot |
| `__wrela_lane1_sum_messages#5` | 1 | hot |
| `__wrela_lane1_sum_messages#6` | 1 | hot |
| `__wrela_lane1_sum_method_hits#0` | 5 | hot |
| `__wrela_lane1_sum_method_hits#1` | 10 | hot |
| `__wrela_lane1_sum_method_hits#2` | 5 | hot |
| `__wrela_lane1_sum_method_hits#3` | 5 | hot |
| `__wrela_lane1_sum_method_hits#5` | 5 | hot |
| `__wrela_lane1_sum_method_hits#6` | 5 | hot |
| `__wrela_lane1_sum_run_one#0` | 1 | hot |
| `__wrela_lane1_sum_run_one#1` | 2 | hot |
| `__wrela_lane1_sum_run_one#2` | 1 | hot |
| `__wrela_lane1_sum_run_one#3` | 1 | hot |
| `__wrela_lane1_sum_run_one#5` | 1 | hot |
| `__wrela_lane1_sum_run_one#6` | 1 | hot |
| `__wrela_lane1_sum_turns#0` | 1 | hot |
| `__wrela_lane1_sum_turns#1` | 2 | hot |
| `__wrela_lane1_sum_turns#2` | 1 | hot |
| `__wrela_lane1_sum_turns#3` | 1 | hot |
| `__wrela_lane1_sum_turns#5` | 1 | hot |
| `__wrela_lane1_sum_turns#6` | 1 | hot |
| `__wrela_lane2_dump#0` | 1 | hot |
| `__wrela_lane2_dump#3` | 1 | hot |
| `__wrela_line_begin#0` | 5 | hot |
| `__wrela_line_commit#0` | 4 | hot |
| `__wrela_line_commit#3` | 4 | hot |
| `__wrela_mb_capacity#0` | 14 | hot |
| `__wrela_mb_capacity#1` | 8 | hot |
| `__wrela_mb_capacity#3` | 6 | hot |
| `__wrela_mb_capacity#4` | 6 | hot |
| `__wrela_mb_core#0` | 7 | hot |
| `__wrela_mb_core#1` | 4 | hot |
| `__wrela_mb_core#3` | 3 | hot |
| `__wrela_mb_core#4` | 3 | hot |
| `__wrela_mb_get_count#0` | 24 | hot |
| `__wrela_mb_get_count#1` | 15 | hot |
| `__wrela_mb_get_count#3` | 9 | hot |
| `__wrela_mb_get_count#4` | 9 | hot |
| `__wrela_mb_get_head#0` | 7 | hot |
| `__wrela_mb_get_head#1` | 4 | hot |
| `__wrela_mb_get_head#3` | 3 | hot |
| `__wrela_mb_get_head#4` | 3 | hot |
| `__wrela_mb_get_tail#0` | 7 | hot |
| `__wrela_mb_get_tail#1` | 4 | hot |
| `__wrela_mb_get_tail#3` | 3 | hot |
| `__wrela_mb_get_tail#4` | 3 | hot |
| `__wrela_mb_has_lineage#0` | 7 | hot |
| `__wrela_mb_has_lineage#1` | 4 | hot |
| `__wrela_mb_has_lineage#3` | 3 | hot |
| `__wrela_mb_has_lineage#4` | 3 | hot |
| `__wrela_mb_load_word#0` | 18 | hot |
| `__wrela_mb_load_word#1` | 12 | hot |
| `__wrela_mb_load_word#3` | 6 | hot |
| `__wrela_mb_load_word#4` | 6 | hot |
| `__wrela_mb_method_count#0` | 23 | hot |
| `__wrela_mb_method_count#1` | 15 | hot |
| `__wrela_mb_method_count#3` | 8 | hot |
| `__wrela_mb_method_count#4` | 8 | hot |
| `__wrela_mb_set_count#0` | 14 | hot |
| `__wrela_mb_set_count#1` | 8 | hot |
| `__wrela_mb_set_count#3` | 6 | hot |
| `__wrela_mb_set_count#4` | 6 | hot |
| `__wrela_mb_set_head#0` | 7 | hot |
| `__wrela_mb_set_head#1` | 4 | hot |
| `__wrela_mb_set_head#3` | 3 | hot |
| `__wrela_mb_set_head#4` | 3 | hot |
| `__wrela_mb_set_tail#0` | 7 | hot |
| `__wrela_mb_set_tail#1` | 4 | hot |
| `__wrela_mb_set_tail#3` | 3 | hot |
| `__wrela_mb_set_tail#4` | 3 | hot |
| `__wrela_mb_slot_words#0` | 14 | hot |
| `__wrela_mb_slot_words#1` | 8 | hot |
| `__wrela_mb_slot_words#3` | 6 | hot |
| `__wrela_mb_slot_words#4` | 6 | hot |
| `__wrela_mb_store_word#0` | 18 | hot |
| `__wrela_mb_store_word#1` | 12 | hot |
| `__wrela_mb_store_word#3` | 6 | hot |
| `__wrela_mb_store_word#4` | 6 | hot |
| `__wrela_mb_turn_index#0` | 14 | hot |
| `__wrela_mb_turn_index#1` | 7 | hot |
| `__wrela_mb_turn_index#3` | 7 | hot |
| `__wrela_mb_turn_index#4` | 7 | hot |
| `__wrela_method_is_aggregate#0` | 11 | hot |
| `__wrela_method_is_aggregate#1` | 4 | hot |
| `__wrela_method_is_aggregate#2` | 3 | hot |
| `__wrela_method_is_aggregate#4` | 1 | hot |
| `__wrela_method_is_aggregate#5` | 1 | hot |
| `__wrela_method_is_aggregate#12` | 7 | hot |
| `__wrela_method_is_aggregate#13` | 7 | hot |
| `__wrela_method_is_aggregate#14` | 3 | hot |
| `__wrela_method_is_aggregate#16` | 4 | hot |
| `__wrela_method_is_aggregate#17` | 2 | hot |
| `__wrela_method_is_aggregate#19` | 2 | hot |
| `__wrela_method_is_aggregate#20` | 2 | hot |
| `__wrela_method_suspends#0` | 11 | hot |
| `__wrela_method_suspends#1` | 4 | hot |
| `__wrela_method_suspends#2` | 3 | hot |
| `__wrela_method_suspends#4` | 1 | hot |
| `__wrela_method_suspends#5` | 1 | hot |
| `__wrela_method_suspends#12` | 7 | hot |
| `__wrela_method_suspends#13` | 7 | hot |
| `__wrela_method_suspends#14` | 3 | hot |
| `__wrela_method_suspends#16` | 4 | hot |
| `__wrela_method_suspends#17` | 2 | hot |
| `__wrela_method_suspends#19` | 2 | hot |
| `__wrela_method_suspends#20` | 2 | hot |
| `__wrela_quiesce_before_halt#0` | 1 | hot |
| `__wrela_quiesce_before_halt#1` | 1 | hot |
| `__wrela_rt_boot_init#0` | 1 | hot |
| `__wrela_rt_boot_init#1` | 3 | hot |
| `__wrela_rt_boot_init#2` | 2 | hot |
| `__wrela_rt_boot_init#3` | 2 | hot |
| `__wrela_rt_boot_init#5` | 2 | hot |
| `__wrela_rt_boot_init#6` | 4 | hot |
| `__wrela_rt_boot_init#7` | 2 | hot |
| `__wrela_rt_boot_init#8` | 2 | hot |
| `__wrela_rt_boot_init#10` | 2 | hot |
| `__wrela_rt_boot_init#11` | 2 | hot |
| `__wrela_rt_boot_init#12` | 1 | hot |
| `__wrela_rt_boot_init#13` | 1 | hot |
| `__wrela_rt_boot_init#18` | 1 | hot |
| `__wrela_rt_checkpoint#0` | 13 | hot |
| `__wrela_rt_checkpoint#12` | 13 | hot |
| `__wrela_rt_checkpoint#13` | 13 | hot |
| `__wrela_rt_checkpoint#14` | 13 | hot |
| `__wrela_rt_checkpoint#15` | 13 | hot |
| `__wrela_rt_checkpoint#16` | 13 | hot |
| `__wrela_rt_checkpoint#18` | 13 | hot |
| `__wrela_rt_checkpoint#19` | 13 | hot |
| `__wrela_rt_enqueue#0` | 7 | hot |
| `__wrela_rt_enqueue#3` | 7 | hot |
| `__wrela_rt_enqueue#6` | 7 | hot |
| `__wrela_rt_enqueue#9` | 7 | hot |
| `__wrela_rt_enqueue#12` | 7 | hot |
| `__wrela_rt_enqueue#13` | 4 | hot |
| `__wrela_rt_enqueue#14` | 7 | hot |
| `__wrela_rt_enqueue#16` | 7 | hot |
| `__wrela_rt_enqueue#18` | 7 | hot |
| `__wrela_rt_primary_entry#0` | 2 | hot |
| `__wrela_rt_primary_entry#1` | 2 | hot |
| `__wrela_rt_primary_entry#6` | 2 | hot |
| `__wrela_rt_primary_entry#7` | 1 | hot |
| `__wrela_rt_primary_entry#9` | 1 | hot |
| `__wrela_rt_primary_entry#10` | 13 | hot |
| `__wrela_rt_primary_entry#11` | 12 | hot |
| `__wrela_rt_primary_entry#13` | 12 | hot |
| `__wrela_rt_primary_entry#14` | 1 | hot |
| `__wrela_rt_primary_entry#15` | 11 | hot |
| `__wrela_rt_primary_entry#22` | 12 | hot |
| `__wrela_rt_primary_entry#23` | 1 | hot |
| `__wrela_rt_primary_entry#24` | 1 | hot |
| `__wrela_rt_run_one#0` | 11 | hot |
| `__wrela_rt_run_one#3` | 11 | hot |
| `__wrela_rt_run_one#4` | 11 | hot |
| `__wrela_rt_run_one#5` | 11 | hot |
| `__wrela_rt_run_one#6` | 11 | hot |
| `__wrela_rt_run_one#8` | 11 | hot |
| `__wrela_rt_run_one#9` | 18 | hot |
| `__wrela_rt_run_one#10` | 18 | hot |
| `__wrela_rt_run_one#11` | 18 | hot |
| `__wrela_rt_run_one#13` | 18 | hot |
| `__wrela_rt_run_one#14` | 18 | hot |
| `__wrela_rt_run_one#15` | 14 | hot |
| `__wrela_rt_run_one#16` | 18 | hot |
| `__wrela_rt_run_one#19` | 18 | hot |
| `__wrela_rt_run_one#20` | 14 | hot |
| `__wrela_rt_run_one#21` | 11 | hot |
| `__wrela_rt_run_one#22` | 7 | hot |
| `__wrela_rt_run_one#23` | 11 | hot |
| `__wrela_rt_run_one#25` | 3 | hot |
| `__wrela_rt_run_one#26` | 7 | hot |
| `__wrela_rt_select#0` | 14 | hot |
| `__wrela_rt_select#3` | 14 | hot |
| `__wrela_rt_select#6` | 14 | hot |
| `__wrela_rt_select#9` | 14 | hot |
| `__wrela_rt_select#10` | 4 | hot |
| `__wrela_rt_select#13` | 4 | hot |
| `__wrela_rt_select#16` | 4 | hot |
| `__wrela_rt_select#17` | 10 | hot |
| `__wrela_rt_select#18` | 3 | hot |
| `__wrela_rt_select#20` | 7 | hot |
| `__wrela_rt_select#21` | 3 | hot |
| `__wrela_rt_select#22` | 7 | hot |
| `__wrela_rt_select#23` | 4 | hot |
| `__wrela_rt_select#24` | 7 | hot |
| `__wrela_rt_select#26` | 7 | hot |
| `__wrela_rt_select#28` | 7 | hot |
| `__wrela_rt_select#29` | 11 | hot |
| `__wrela_rt_select#32` | 11 | hot |
| `__wrela_rt_select#35` | 11 | hot |
| `__wrela_rt_select#40` | 11 | hot |
| `__wrela_rt_select#41` | 7 | hot |
| `__wrela_rt_select#42` | 4 | hot |
| `__wrela_rt_select#44` | 3 | hot |
| `__wrela_rt_select#46` | 3 | hot |
| `__wrela_rt_select#47` | 3 | hot |
| `__wrela_rt_select#49` | 4 | hot |
| `__wrela_rt_select#50` | 4 | hot |
| `__wrela_rt_select#51` | 4 | hot |
| `__wrela_rt_summary_and_halt#0` | 1 | hot |
| `__wrela_rt_summary_and_halt#2` | 1 | hot |
| `__wrela_select_count#0` | 11 | hot |
| `__wrela_select_count#1` | 11 | hot |
| `__wrela_select_root#0` | 14 | hot |
| `__wrela_select_root#1` | 14 | hot |
| `__wrela_select_root#2` | 7 | hot |
| `__wrela_select_root#4` | 7 | hot |
| `__wrela_select_root#5` | 7 | hot |
| `__wrela_test_append_prefix#0` | 1 | hot |
| `__wrela_test_append_prefix#1` | 1 | hot |
| `__wrela_test_call#0` | 2 | hot |
| `__wrela_test_call#1` | 2 | hot |
| `__wrela_test_suspends#0` | 1 | hot |
| `__wrela_test_suspends#1` | 1 | hot |
| `__wrela_test_turn_index#0` | 12 | hot |
| `__wrela_test_turn_index#1` | 12 | hot |
| `__wrela_try_drain#0` | 11 | hot |
| `__wrela_try_drain#1` | 11 | hot |
| `__wrela_try_select#0` | 14 | hot |
| `__wrela_try_select#3` | 14 | hot |
| `ascii_digit#0` | 14 | hot |
| `ascii_digit#3` | 14 | hot |
| `ascii_digit#4` | 5 | hot |
| `ascii_digit#6` | 9 | hot |
| `ascii_digit#7` | 3 | hot |
| `ascii_digit#9` | 6 | hot |
| `ascii_digit#10` | 3 | hot |
| `ascii_digit#12` | 3 | hot |
| `ascii_digit#13` | 1 | hot |
| `ascii_digit#15` | 2 | hot |
| `ascii_digit#18` | 2 | hot |
| `ascii_digit#21` | 2 | hot |
| `ascii_digit#22` | 2 | hot |
| `copy_bytes_range#0` | 1 | hot |
| `copy_bytes_range#1` | 13 | hot |
| `copy_bytes_range#2` | 12 | hot |
| `copy_bytes_range#3` | 12 | hot |
| `copy_bytes_range#5` | 12 | hot |
| `copy_bytes_range#6` | 1 | hot |
| `copy_line_buf_range#0` | 33 | hot |
| `copy_line_buf_range#1` | 122 | hot |
| `copy_line_buf_range#2` | 89 | hot |
| `copy_line_buf_range#3` | 89 | hot |
| `copy_line_buf_range#5` | 89 | hot |
| `copy_line_buf_range#6` | 33 | hot |
| `turns#0` | 1 | hot |
| `turns#1` | 1 | hot |
| `turns#8` | 1 | hot |
| `turns#9` | 1 | hot |
| `turns#16` | 1 | hot |
| `turns#17` | 1 | hot |
| `turns#18` | 1 | hot |
| `turns#19` | 1 | hot |
| `turns#20` | 1 | hot |
| `turns#22` | 1 | hot |
| `turns#29` | 1 | hot |

---

## 6. Item D's entry point and failure modes

**Call `wrela_compiler::cost::layout_classes(source: Option<&Path>, spans:
&[BlockSpan]) -> Result<LayoutClasses, String>`**, where `spans` is
`codegen::block_spans()` from a build under `codegen::set_block_bridge(true)`
(bridge mode emits not one extra word). Then
`LayoutClasses::class_of(fn_key, block_index) -> BlockClass`.

- **No sidecar beside the source (or no source) → `Ok(LayoutClasses::
  Unmeasured)`.** Every `class_of` answers `BlockClass::Unmeasured`. **D
  must read that as "everything hot, layout unchanged"** — not as a guess at
  coldness. This is the only silent path, and it is silent because "no
  measurement" is the ordinary case for every non-boot-bearing program in
  the tree.
  Oracle: `unit:missing_sidecar_degrades_to_unmeasured_never_to_a_guess`.
- **Sidecar present but malformed, or stale → `Err`.** A stale profile is a
  wrong profile and must not lay out an image.
- **`BlockClass::Unmeasured` must never be treated as `Cold`.**
  `MeasuredBlocks::is_hot` in `cost::bridge` *does* return false for
  unmeasured — that is correct for the footprint term (a documented
  under-count) and **wrong for layout**. Do not reuse it for D.

### What a real build actually looks like

`unit:layout_classes_over_a_real_bridge_mode_build` runs the whole entry
point against a real bridge-mode cost-stage build of `boot-actors` (0.08 s
for the full 13-unit module, so it stays in the cheap lane):

| | |
| --- | --- |
| sidecar fns matched by the built closure | **14** of 67 |
| sidecar fns absent from it | **53** |
| sidecar keys that resolve | **81** of 364 |
| built blocks classified hot / cold / unmeasured | **81 / 85 / 18** of 184 |

The committed sidecar is **not** stale against a fresh build. The 81
resolving keys independently reproduce M20's own "81 resolve" figure
(line 911), so this unit cross-checks that number rather than restating it.

---

## 7. Decisions 1720–1726, each with its oracle

| # | Decision | Oracle |
| --- | --- | --- |
| **1720** | Derivation reads the sidecar alone — no CFG, no second closure. The sidecar's key space is the `@test(runtime)` image (2527 blocks) and no in-compiler build reproduces it (the cost-stage closure has 184), so a CFG-based derivation would have to rebuild and re-partition the test image inside a unit test. It does not need to: every number here is provable from the counts alone. | `unit:derives_the_committed_boot_actors_vector` |
| **1721** | `f(fn#0)` is the fn's call count; for an async fn, *fresh entries only*. | `unit:call_freq_agrees_with_lane1_on_the_sync_methods` — Lane 1 counts **turns**, so the two must agree exactly on sync methods and differ by exactly the suspension count on async ones. `Ledger.mark` 3/3 and `Ledger.read_marks` 1/1 (sync, exact); `Worker.slow` 3 turns / 1 fresh, `Worker.quick` 2/1, `Worker.report` 2/1. |
| **1722** | A block is loop-resident iff `f(b) > f(fn#0)` — **proved, not guessed**: a block cannot run more than once per invocation without a back-edge. Trip count is `f(b)/f(fn#0)`, carried as integer milli-trips (no float on an output path). Grain is a contiguous run, honestly labelled. | `unit:the_peak_measured_trip_count_is_thirteen` |
| **1723** | Hot/cold is three-valued; **`Unmeasured` ≠ `Cold`**. | `unit:hot_cold_is_three_valued_and_unmeasured_is_not_cold` (synthetic, all three classes) + `unit:layout_classes_over_a_real_bridge_mode_build` (81/85/18 over a real partition) |
| **1724** | `__wrela_rt_primary_boot` excluded by name as an instrumentation artifact; removed mass reported in `artifact_hits`; an artifact-only vector is an error. | `unit:derives_the_committed_boot_actors_vector` (pins 6647 / 3938 / 67 / 364) + `unit:an_artifact_only_sidecar_is_not_a_measurement` |
| **1725** | Staleness **fails closed** in three directions: (1) an out-of-range block index for a fn present in both; (2) zero fns in common; (3) an empty partition — the caller did not build under bridge mode, and a silent pass would classify an image against nothing. | `unit:a_stale_sidecar_fails_closed_on_a_shrunken_fn`, `unit:a_sidecar_for_a_different_program_fails_closed`, `unit:a_present_sidecar_over_an_unbuilt_partition_fails_closed` |
| **1726** | Item D's entry point is `cost::layout_classes`, with the failure modes in §6. | `unit:layout_classes_over_a_real_bridge_mode_build` |

### The honest residual on 1725

Drift that keeps every measured index **in range** is invisible to all
three checks above. That is what `PartitionCheck::partition_digest` is for
— FNV-1a over the built `(fn_key, block_count)` pairs of the measured fns,
a number a caller pins. `unit:in_range_drift_is_invisible_to_the_key_checks_
and_visible_in_the_digest` pins exactly that limit, asserting that
`matched_keys` does **not** move and the digest does, so the gap cannot be
quietly forgotten.

---

## 8. Two claims from item A's first pass that the real build falsified

Recorded because the plan should not inherit them:

1. **"Only 8 of the sidecar's 67 fns exist in the cost-stage closure."**
   Wrong — it is **14** (`unmatched_fns` 53). The 8 came from an
   intermediate count over scored fns, not over the recorded partition.
2. **"Unmeasured blocks dominate a real closure, so a cold-by-default
   reading would sink most of the program."** Wrong, and this one was
   load-bearing for how decision 1723 was argued. Measured: **18** of 184
   blocks are unmeasured; the closure is 90% covered. Decision 1723 stands
   on the narrower ground now written into the code: 18 blocks would be
   sunk on no evidence whatever, and a layout pass has no way to know in
   advance whether it is looking at this closure or at one the
   `@test(runtime)` image barely covers — nothing in the vector announces
   which.

---

## 9. Verification

Cheap lane, per the plan's split:

```
cargo test -p wrela-compiler --lib cost::derive
→ 13 passed; 0 failed; finished in 0.08s
cargo fmt -p wrela-compiler                 → clean
cargo clippy -p wrela-compiler --lib        → no warnings in derive.rs
```

Every unit exercises the new path (freeze 1714): determinism (same input →
byte-identical render; reversed input line order → identical render; digest
stable), the three fail-closed directions, the in-range residual, the
three-valued classification, and the end-to-end entry point over a real
build.

**`diff-eval`: skipped, deliberately.** Item A adds a module and changes
nothing the pipeline consumes — no codegen, layout, or emission path was
touched, so there is no evaluator-vs-backend disagreement it could expose.

**Focused boot (`boot-actors` under `--block-count`): not run.** Expected
result is *no change at all* — a byte-identical transcript, because nothing
in the pipeline calls this module. A diff in that transcript would mean the
derivation somehow reached emission, which would be a bug in this item
rather than a measurement.
