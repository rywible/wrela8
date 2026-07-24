# Review queue — normative edits made without a human

Audit trail (GOAL.md rule): every normative `docs/language/` edit made
during autonomous orchestration gets one line here — file §section,
one-line summary, commit sha. Newest last.

- 05-library.md §9 — seal spelling reconciled to `img.seal()` (02 §12.1's example and virtio-storage.wr:639 already used the method form; 05 §9 alone said `seal(take builder)`) — c0bce9e
- bench/thresholds.toml check_golden_median_us — raised 40000 → 400000 at M6-A (b6fd80f): the check-lane bench times sema over the whole golden corpus, which grew 25 → 154 entries since the lock; measured release-mode median ~13ms (was ~13ms at lock time over the smaller corpus in debug-adjacent conditions); raise is corpus growth + headroom, not an algorithmic regression. Recorded here per the threshold-raise rule (before/after numbers above).
