# Review queue — normative edits made without a human

Audit trail (GOAL.md rule): every normative `docs/language/` edit made
during autonomous orchestration gets one line here — file §section,
one-line summary, commit sha. Newest last.

- 05-library.md §9 — seal spelling reconciled to `img.seal()` (02 §12.1's example and virtio-storage.wr:639 already used the method form; 05 §9 alone said `seal(take builder)`) — c0bce9e
- bench/thresholds.toml check_golden_median_us — raised 40000 → 400000 at M6-A (b6fd80f): the check-lane bench times sema over the whole golden corpus, which grew 25 → 154 entries since the lock; measured release-mode median ~13ms (was ~13ms at lock time over the smaller corpus in debug-adjacent conditions); raise is corpus growth + headroom, not an algorithmic regression. Recorded here per the threshold-raise rule (before/after numbers above).
- 02-language.md §12.2 — @test(runtime) fns may declare Actor[T] params; runner supplies the unique declared instance's handle (M6's execution surface needed a doc-backed handle path) — f6161f3
- OBSERVATION, no code change (M6 item H) — `--stage=mwir` and `--stage=flowwir` each silently omit the half of the program the other owns: on a mixed sync+async file, mwir renders only the sync fns and flowwir only the async ones, with no marker saying the rest exists elsewhere. `--stage=asm` (the union) is complete, and the partition predicate (`TypedFn::is_async`) is identical on both sides, so no fn can currently fall out of both — this is a review-surface honesty question, not a live defect, which is why it was recorded rather than coded (GOAL.md: extra ideas outside exit criteria go here, never into code). Worth a human call because the golden diff *is* the review surface: a future misclassification would move a fn between two goldens and neither diff would look like an error. Cheapest dumb fix if wanted: each dump prints a trailing one-line-per-key section naming the fns the other stage owns.
- 06-machine.md §1 (+ README / 01-model / 04-compiler core-count sentences) — 4 → 3 vCPUs per ROADMAP 2026-07-24; stacks region 4 MiB → 3 MiB; ledger `machine.cpu.three-vcpus` — 28a9bc1
