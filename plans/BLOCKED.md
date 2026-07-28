# Blocked work

## M15 item K — barrier-deletion mutation oracle (2026-07-27)

**What is needed:** A host that observes store-store / load-load reordering
across a cross-core ring when `@dmb(ishst|ishld)` words are stripped
(`wrela test --omit-dmb`), so the mutated arm of
`golden/boot-cross-core-publish-acquire` fails its checksum under concurrent
record (plans/M15.md decision 9 / known risk 4).

**What was done:**
- Inline `@dmb` on publish/drain (item H).
- True overlapping `hv_vcpu_run` + Yield-`Progress` replay (item I).
- `--omit-dmb` front-door strips every `Inst::Dmb` (unit + image-byte proof).
- Intact arm of `boot-cross-core-publish-acquire` → `ok` under overlap.
- Mutated arm also → `ok` at `ITERS=65536` ×3 on macOS/HVF (this machine).

**Likely cause:** await-RPC park/wake goes through the VMM, which serializes
store visibility before the consumer drains; Apple Silicon may also be
stronger than the architectural weak model for these DRAM stores.

**Prescribed disposition (M15 known risk 4):** fail closed; do **not** weaken
to "barriers present in asm only." Clause
`machine.cross-core.publish-acquire-barrier` stays `gap`. Flip when a test
host observes the mutated checksum failure.

**How to resume:** Re-run intact + `--omit-dmb` arms of
`boot-cross-core-publish-acquire` on a host that exhibits the reordering;
pin `expected/test-omit-dmb.txt` as a failed `@test`; flip the clause.
