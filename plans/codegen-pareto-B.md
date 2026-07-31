# codegen-pareto item B — Addressing wins

Working record for item B of [codegen-pareto.md](codegen-pareto.md), per
decision 1709. Decision block **1730–1739**.

## Verdict per sub-item

| Sub-item | Outcome |
| --- | --- |
| **B1** `ADR`-only rodata addressing | **Landed** as `OptId::AdrAddressing`. Passes the ∀ gate over the whole `cost-*` corpus, marginally over the previous release list: `outcome=wins_at_every_point points_per_side=24576` across 15 cases. |
| **B2** `ADR` for placed statics | **Not attempted.** Reasoned below; it is a boot-verified change and this round had no boot lane. |
| **B3** fold `add xN, sp, #off` into addressing modes | **Not applicable — measured, not assumed.** The pattern does not exist in this backend: **0** of 5785 `add xN, sp, #off` sites are followed by a load or store through the computed base. |
| **B4** branch-to-branch / branch-to-fallthrough cleanup | **Built, measured, reverted.** It wins on every one of the 15 cost cases, and it is incompatible with plans/M20.md decision 1608's bridge contract in a way item B may not resolve. Full write-up below, including the conservative rule that would make it land. |

Nothing in this item tuned a cost-model row. `ADR` reuses `CostRule::Adrp`
— same encoding family, same A76 port, same latency — so every number below
is a word the scoreboard no longer issues, never a re-priced one. The
committed `bench/a76-pi5.toml` is untouched; the sweep reports the same
`table_digest` on both sides.

## Decisions

**1730. B1 lands as a named opt, not an unconditional form change.** Freeze
1714 says nothing lands without passing the ∀ gate, and the gate compares
two *opt lists* — an unconditional change is invisible to it, so it could
not be ranked at all. `dev` keeps the `ADRP`+`ADD` pair as the reference
form, which is the same shape M19 freeze 1407 already uses for
`NarrowImm`.

**1731. The range proof is `layout::patch_adr`, and it runs on real
addresses.** Codegen commits to one word; layout measures every site's
`target − this` against `ADR`'s own signed `imm21` (±1 MiB) before writing
a word. There is no path that emits an `ADR` without passing through it —
`enc_adr` has exactly one production caller pair
(`FnCtx::load_rodata_addr` / `push_rodata_addr`) and one patcher.

**1732. An out-of-range site is a hard build error, not a reported
fallback.** Freeze 1713 allows either, provided it is loud and tested. A
fallback is not available here even in principle: the `ADRP`+`ADD` pair is
two words where codegen already committed one, so widening at layout time
would move every address the pass has already patched and every section
size `verify_section_sizes` is about to check. The honest alternatives were
"prove and error" or "iterate layout to a fixpoint"; the second is a whole
new pass shape for a condition sitting 11× away from firing (see the
headroom table). So it errors, and the error names the site, the target,
the distance, the ±1 MiB bound, and the knob that turns the substitution
off.

**1733. `AdrAddressing` is appended to `RELEASE_OPTS`, not inserted.** The
opts are mutually independent TLS knobs — `BoundsElide` deletes lower-side
bounds checks, `NarrowImm` shortens constant materialization,
`AdrAddressing` shortens rodata address materialization, and none changes
what another sees. Order is therefore convention, and appending leaves
decision 1423's pinned pair exactly as it was.
`unit:swapped_order_scores_same_as_release_opts` still holds over all
three.

**1734. B4's transform is a *plan*, not a peephole.** (Reverted with B4;
recorded because the shape is the reusable part.) `emit_fn` /
`emit_flowwir_fn` size the code in one pass and write it in another, and
`debug_assert_eq!(ctx.words.len(), word_offsets[body.len()])` says the two
must agree exactly. A word deleted *after* emission moves every later word
and invalidates every reloc index, every branch displacement and every Lane
2 span. So the decision is made before either pass, out of information both
passes have — body indices — and both passes then make it identically for
free. The two transforms key off different tables: chase-through is keyed
by *declared target* (so conditional branches read it too) and is legal
only through an index that emits nothing *but* its branch; elision is keyed
by *branching index* and needs every index between it and its target to
emit nothing.

**1735. Lane 2 counting builds get no B4.** `--block-count` emits a counter
per block; chaining a jump past a block skips that block's counter, so the
reported frequencies would describe a program that is not the one that ran.

**1736. Lane 2 *bridge* builds cannot get B4 either — and that is what
stops B4 landing.** See "B4: why it did not land".

**1737. B3 is closed as not-applicable on a measurement, not descoped.**
The plan assumed the fold existed; it does not. Recorded below with the
count so the next reader does not re-derive it.

**1738. B2 is deferred to a round with a boot lane.** Reasoned below.

## B1 — what landed

`Reloc::RodataAdr { word, byte_offset }` joins `Reloc::Rodata`. A separate
variant rather than a `form:` field, because the two carry different
contracts: `Rodata`'s `word_adrp + 1` must exist and must be the paired
`ADD`; `RodataAdr`'s must not be assumed to be anything. Two variants let
the type say that; one variant with a discriminant makes every reader
re-derive it.

Three emission sites, all now one word under the opt:
`FnCtx::load_rodata_addr` and the two hand-assembled abort stubs, which
were folded onto one `push_rodata_addr` helper (the helper takes the
already-formatted offset text so both `dev` dumps stay byte-identical to
what they were).

Four resolution sites patched: `layout.rs`'s reloc loop and all three of
`layout/harness.rs`'s (harness section, code section, and
`shift_reloc_words`).

### The range proof and its failure mode

`ADR_HALF_RANGE_BYTES = 1 << 20`. `patch_adr` refuses on
`!(-2^20..2^20).contains(&(target - this))`, leaves the placeholder
untouched, and returns:

```
relocation out of range: an `ADR` at 0x40500000 targets 0x40600000, 1048576
bytes away — outside `ADR`'s own ±1 MiB (±1048576 byte) reach.
`OptId::AdrAddressing` (plans/codegen-pareto.md item B) substitutes one
`ADR` for an `ADRP`+`ADD` pair only where the whole image proves every site
is in reach; this image is too large between its code and its rodata for
that. Build in `dev`, or drop `OptId::AdrAddressing` from `RELEASE_OPTS`,
to get the two-word page-relative form back.
```

Pinned by
`unit:patch_adr_out_of_range_fails_the_build_rather_than_emitting_a_wrong_adr`,
which checks both directions **and** both last-in-range edges, so the bound
is a bound and not an off-by-one moat.

### Measured headroom (decision 1703's claim, re-derived here)

Worst code-to-rodata span over all 62 pinned images that have both
sections, measured as *first code word → last rodata byte* (the furthest
any site can be):

| image | span | of ±1 MiB |
| --- | --- | --- |
| `boot-receipt-handoff` | 99 262 B (0.095 MiB) | 10.6× inside |
| `boot-receipt-recover` | 96 999 B (0.093 MiB) | 10.8× inside |
| `boot-dma-reclaim` | 96 791 B (0.092 MiB) | 10.8× inside |

Decision 1703 quoted ~99 KB for `boot-receipt-handoff`; that reproduces
exactly. B1 itself shrinks these further (see the image numbers below), so
the headroom only grows.

### Numbers — the `cost-*` corpus

Marginal: `RELEASE_OPTS` **without** `AdrAddressing` → `RELEASE_OPTS`, at
the pinned point. Words saved is one per rodata site, counted as the `adrp`
count of the baseline emission.

| case | cycles before | cycles after | Δ | words saved |
| --- | ---: | ---: | ---: | ---: |
| cost-align | 331 | 331 | 0 | 0 |
| cost-arith | 153 | 147 | −6 | 6 |
| cost-assoc-conflict | 959 | 959 | 0 | 0 |
| cost-bounds-elide | 314 | 314 | 0 | 0 |
| cost-branch-bias | 522 | 511 | −11 | 11 |
| cost-branchy | 172 | 170 | −2 | 2 |
| cost-calls | 90 | 88 | −2 | 2 |
| cost-crosscore | 4319 | 4278 | −41 | 71 |
| cost-forwarding | 341 | 341 | 0 | 0 |
| cost-icache-cliff | 29 583 | 28 083 | −1500 | 1522 |
| cost-itlb-span | 74 079 | 70 227 | −3852 | 3874 |
| cost-mem-locality | 169 | 169 | 0 | 0 |
| cost-mpipe-block | 630 | 612 | −18 | 18 |
| cost-ports | 381 | 381 | 0 | 0 |
| cost-runtime | 1954 | 1924 | −30 | 52 |
| **total** | **113 997** | **108 535** | **−5462 (−4.79 %)** | **5558** |

Six cases are flat because they emit no rodata reference at all (no checked
arithmetic, no bounds check, no string literal — nothing to abort with).
Flat is not a rise, and the gate's rule is "no case may rise at any point,
at least one must fall at every point".

The two budget-witness cases also lower the **priced** I-side charge, which
is the term that replaced the words veto (decision 1619):
`cost-icache-cliff` 2982 → 2331 and `cost-itlb-span` 18 463 → 16 572. So
this opt is visible to the gate on both terms, not only on cycles.

Per-opt attribution against `dev` (whole corpus totals, from
`unit:narrow_imm_wins_on_cycles_while_its_footprint_win_is_priced_at_zero`,
which item B extended to three singles): dev 124 637 → `AdrAddressing`
alone 119 069 (**−5568**), against `BoundsElide` −4592 and `NarrowImm`
−6472; `release` −16 102, bounded by the sum of the singles
(4592 + 6472 + 5568 = 16 632) as the structural claim requires.

### Numbers — a real sealed image (`image-basic`)

| | before | after |
| --- | ---: | ---: |
| `code` section | 82 576 B | 80 520 B (**−2056 B, −514 words**) |
| hot text (core 0) | 89 728 B | 87 488 B |
| priced budget charge | 2646 | 2401 |
| image cost total | 29 426 | 29 134 |
| rodata base | 0x405142e0 | 0x40513ad8 |

### The ∀ gate verdict

Whole corpus, `#[ignore]`d deep lane, run once at the end of this item:

```
outcome=wins_at_every_point points_per_side=24576
AdrAddressing ∀-sweep: 24576 points/side over 15 cases
test opts::win::tests::adr_addressing_wins_at_every_point_of_the_residual_box ... ok
test result: ok. 1 passed; 0 failed; ... finished in 451.08s
```

Baseline `[BoundsElide, NarrowImm]`, candidate `RELEASE_OPTS`. Box is 17
dimensions / 131 072 nominal points; surviving `k` per case is 8–14, so
24 576 enumerated corners per side. No veto of any kind — no rise at any
point, no coverage fall, no budget growth, no ordering-word removal.

Two smoke forms run in the default `cargo test` lane, on `cost-arith` (the
smallest case that emits rodata references at all, six of them):
`adr_addressing_wins_at_every_box_point_on_the_smoke_case` (vs `dev`) and
`adr_addressing_is_a_marginal_win_over_the_previous_release_list` (the one
that actually decides the landing — it holds the other two opts fixed on
both sides, so the win cannot be another opt's coattails).

### `diff-eval`

Run once, at the end, verbatim tail:

```
diff-eval: 121 test(s) agree across 46 case(s), 8 lowering-skips, 6
exhaustive-skips, 1 quota-skips, 0 import-skips
```

Exit code 0. No disagreement.

## B3 — not applicable, and here is the measurement

Scanned every `--stage=asm` dump of the `cost-*` and `asm-*` families
(89 022 lines):

- `add xN, sp, #off` sites: **5785**
- of those, followed within three instructions by a load or store whose
  base is `xN`: **0**
- of those, with *any* consumer of `xN` within ten instructions: 81 —
  45 `ldr`, 32 `str`, 2 `mov`, 2 `add`, and never at distance 1.

The reason is structural, not accidental. `FnCtx::load_slot` /
`store_slot` already emit `[sp, #off]` directly; the frame model never
materializes a slot address in order to dereference it. `addr_of_slot`
exists only where the *address itself is the value* — an aggregate passed
to a call by pointer, or an array base before index scaling — and those
feed a `bl` (whose argument register does not appear in the dump text) or
an index-scaling `add`, neither of which is an addressing mode.

There is nothing to fold. Building the fold anyway would be complexity
with a measured zero on the other side, which is what the cleverness budget
exists to refuse.

## B4 — built, measured, reverted

### What it was

`OptId::BranchCleanup`, implemented as decision 1734's plan: one
`JumpPlan { chase, elide }` per fn, computed before either codegen pass
from the body's index space, consulted by the `Inst::Jump` and
`Inst::Return` arms of `emit_one` (both the sync body and the async flat
stream, which share that arm through `Transition::Return`).

The real customers were **`Return`s**, not `Jump`s. Every `Return` branches
to the shared epilogue sentinel, and the trailing `Return` of every fn
branches to the word immediately after it — plus every `Return` that only
trailing `Return`s separate from the epilogue. Across the `cost-*` and
`asm-*` corpus, 357 of 791 unconditional `b` words (45 %) were
branch-to-fallthrough and 34 more were branch-to-branch.

### What it measured

Marginal over `RELEASE_OPTS` as landed (i.e. on top of B1), at the pinned
point. **Every one of the fifteen cases falls:**

| case | before | after | Δ |
| --- | ---: | ---: | ---: |
| cost-align | 331 | 296 | −35 |
| cost-arith | 147 | 100 | −47 |
| cost-assoc-conflict | 959 | 937 | −22 |
| cost-bounds-elide | 314 | 303 | −11 |
| cost-branch-bias | 511 | 485 | −26 |
| cost-branchy | 170 | 157 | −13 |
| cost-calls | 88 | 53 | −35 |
| cost-crosscore | 4278 | 4123 | −155 |
| cost-forwarding | 341 | 317 | −24 |
| cost-icache-cliff | 28 083 | 27 924 | −159 |
| cost-itlb-span | 70 227 | 70 002 | −225 |
| cost-mem-locality | 169 | 147 | −22 |
| cost-mpipe-block | 612 | 552 | −60 |
| cost-ports | 381 | 328 | −53 |
| cost-runtime | 1924 | 1820 | −104 |
| **total** | **108 535** | **107 544** | **−991 (−0.91 %)** |

On the small cases it is 10–40 %: `cost-calls` 88 → 53, `cost-arith`
147 → 100.

### Why it did not land

Lane 2's block bridge (plans/M20.md item C, **decision 1608**) checks that
two partitions of each fn agree before any measured frequency is
attributed: the **MWIR** block partition that owns the block ids, and the
**emitted-word** partition that owns `s(b)`. Rule 4 of that check is that
every Lane 2 span boundary must be an emitted-word block leader, and 1608
forbids falling back to nearest-offset attribution.

Eliding a branch **merges two emitted-word blocks** while leaving the MWIR
partition exactly as fine as it was. That is precisely the disagreement
1608 fails closed on — and it did, loudly:

```
boot-actors: bridge must agree: bridge: fn `Ledger.mark` block 0 ends at
word 42 which is not an emitted-word block leader (decision 1608: never
attribute by nearest offset)
```

Finding that by construction rather than by a wrong number is the check
working, so nothing here is a complaint about it.

Two ways out, and item B may take neither:

1. **Turn the transform off in bridge mode**, the way decision 1735 turns
   it off under `--block-count`. This breaks a *stronger*, already-pinned
   invariant —
   `unit:block_bridge_mode_leaves_the_word_stream_byte_identical`, which
   says the bridge is a pure observer. A bridge that changes the code it
   measures is not a bridge. Tried; it turns one red test into a different
   red test, and the second one matters more.
2. **Relax the bridge to accept a merged boundary.** That is a change to
   another milestone's frozen decision, made from inside a parallel item,
   with no boot lane to check it against. Out of scope by construction.

### The rule that would make it land

Recorded so the next attempt starts from the finding rather than
re-deriving it. Make the transform **Lane-2-boundary-preserving
unconditionally** — computing the MWIR leader set from `f.body` in every
mode, not only when ids are assigned — so release and bridge emit
identically and both partitions still line up:

- **Drop the chase entirely.** Retargeting removes a branch reference from
  its old target, and a Lane 2 leader whose only reference was that branch
  stops being a word leader. Branch targets are leaders by definition, so
  a boundary-preserving chase is nearly empty anyway. It was 34 sites of
  the ~391.
- **Elide only when the merge stays inside one Lane 2 block**: refuse if
  `i + 1` is a Lane 2 leader (that leader's word-start owes its
  leader-hood to the branch being deleted). The trailing-`Return` case —
  where the target is the epilogue sentinel past the end of the body, so
  no Lane 2 span starts at it — is the one that survives, and it is also
  the most common.

That is a real design, not a hand-wave, but it needs a boot transcript to
be believable (it changes the control flow of every fn in every image) and
decision 1708 gives this round's boot lanes to the orchestrator. It belongs
in a round that can run one.

## B2 — deferred, with the reason

Not attempted, deliberately (decision 1738).

B2 is not the same shape as B1. B1 converts **one** reloc class with one
target section. B2 converts a *classification*: ten-odd reloc variants
(`TurnFrameAddr`, `TurnsBase`, `GroupArenaBase`, `WakePending`,
`MailboxAddr`, `RrCursor`, `RingAddr`, `DriverState`, `PoolBase`,
`PoolSlot`) whose four-word `load_imm` shape is emitted from `codegen.rs`
*and* hand-assembled in `layout/harness.rs`, `layout/boot_init.rs` and
`layout/rtdata.rs`, and each has to be sorted into "targets rtdata /
pooldata, ~256 KB out, in `ADR` range" versus "targets `pages`
(0x40000000, ~5 MB out) or a real device MMIO window, stays absolute" —
decision 1703 names that split but does not enumerate it.

Every one of those addresses is dereferenced by a booting image on its
first turn. The oracle for getting the classification right is a boot
transcript, and this round has none available to me. Landing it on unit
tests alone would be claiming a verification I did not do. The value is
real — 4 words → 1 at every site, a bigger per-site win than B1's 2 → 1 —
so it should be picked up by a round that can boot, with the enumeration
above as its starting checklist.

## What is left undone

- **B2**, deferred with a reason and a checklist (above).
- **B4**, reverted with a measured win, a named blocker, and the rule that
  would unblock it (above).
- **B3**, closed as not-applicable on a measurement.
- Boot transcripts and the full `cargo xtask check` for B1 — the
  orchestrator's, per decision 1708. B1 moves `--stage=asm`, `--stage=cost`,
  `--stage=image` and `--stage=report` output for every case that emits a
  rodata reference; the non-booting expectations are re-pinned in B1's own
  commit, the booting ones are not.
