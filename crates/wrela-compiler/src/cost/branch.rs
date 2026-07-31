//! The branch model (plans/M20.md item H, decisions 1604 / 1606 / 1609,
//! `compiler.costs.branch-bias`): a **bias-derived** mispredict charge plus
//! the two SOG §4.8 front-end terms a per-fn word stream can actually
//! decide.
//!
//! ## Why a mispredict penalty may be nonzero at all
//!
//! The pre-M20 table pinned `branch_penalty = 0` deliberately, and the
//! reasoning was sound: a scoreboard has no predictor, so it can never
//! argue that a branch **is** mispredicted, and charging the penalty
//! unconditionally would over-credit branch *removal* — if-conversion would
//! read as a multi-cycle win when a well-predicted branch costs the real
//! machine nothing. Per-block `f` dissolves that. A branch's two successors
//! have measured counts, so the branch has a measured taken ratio, and the
//! charge is scaled by how *unpredictable* that ratio is: ~0 for a
//! well-predicted branch, full for a near-50/50 one. Deleting a predictable
//! branch then earns ~0, which is correct. **The penalty is chargeable
//! because block-grain `f` (item C) exists** — that is the whole
//! justification for the term, and it is why item H gates on item C.
//!
//! ## The scaling function, and why it is a bound rather than a simulation
//!
//! For a branch whose target block was entered `t` times and whose
//! fallthrough block was entered `n` times, with `T = t + n`:
//!
//! ```text
//! unpredictability u = 2 · min(t, n) / T          (0 at t=0 or n=0, 1 at t=n)
//! charge           c = ceil(mispredict_penalty × u)
//! ```
//!
//! `min(t, n) / T` is exactly the mispredict rate of the **majority-direction
//! predictor** — the best any predictor can do knowing only this branch's
//! marginal bias. A history-based predictor (which A76 has) can only do
//! *better* on a patterned stream: an alternating branch has ratio 0.5 and
//! is perfectly predictable. So the term is an **upper bound on the
//! mispredict rate obtainable from the datum the model holds**, doubled only
//! to normalise the endpoint plans/M20.md fixes ("full at 0.5"). The `ceil`
//! is decision 1609: round toward over-costing.
//!
//! That bound is **not** a predictor simulation (decision 1606 / freeze
//! 1622), and the difference is visible in the code rather than asserted:
//!
//! - there is no predictor state anywhere — no table, no history register,
//!   no update rule, no per-execution sequence, nothing that is mutated as
//!   words are walked;
//! - [`branch_mispredict_charge`] is a **pure function** of
//!   `(penalty, taken, not_taken)`. The same branch scored twice yields the
//!   same number regardless of order, context, or what was scored before —
//!   which a predictor simulation could never promise, because its answer
//!   is a function of history;
//! - the term never claims to know *which* execution mispredicts, only how
//!   many mispredicts the marginal bias can force.
//!
//! ## No data is not a 50/50 — and that is the subtle part
//!
//! Under `W_flat` (`f ≡ 1`) every block has count 1, so a naive taken ratio
//! is `1/(1+1) = 0.5` — *maximum* unpredictability — and the model would
//! charge the **full** penalty on every branch in the program. That is
//! backwards. plans/M20.md item H is explicit: under `W_flat` there is no
//! bias information, so **the flat row charges zero mispredict** (decision
//! 1609 applied honestly — no data, no charge), which also means a
//! frequency-dependent branch opt cannot be argued on the flat row.
//!
//! So "no data" is made **structurally** distinct from a measured 0.5,
//! three ways, none of which is a convention a later edit can drift past:
//!
//! 1. [`BlockCounts::Flat`] short-circuits in [`BlockCounts::obs`] and
//!    returns `None` **without consulting any count**. There is no code
//!    path from `f ≡ 1` to a ratio: the flat row cannot produce a
//!    [`BranchBias`] at all, rather than producing one that happens to
//!    scale to zero.
//! 2. [`BranchBias`]'s fields are private and its only constructor,
//!    [`BranchBias::from_observations`], takes two `Option<BlockObs>` and
//!    returns `Option<Self>`. A missing side is `None`, full stop.
//! 3. Two observations that carry the **same** [`BlockObs::source`] are one
//!    datum, not a ratio, and also yield `None`. This is load-bearing, not
//!    defensive: item C's Lane 2 ids are assigned over *MWIR* blocks while
//!    `s(b)` is computed over *emitted-word* blocks, and one MWIR block
//!    covers several emitted-word blocks — so a branch whose target and
//!    fallthrough both live inside one Lane 2 span would otherwise read
//!    `1:1` and be charged the full 14 cycles. Every abort branch has
//!    exactly that shape.
//!
//! ## What an unmeasured branch is charged
//!
//! **Nothing** — the same "no data, no charge" as the flat row. Item C
//! measured block-grain coverage on `boot-actors` at 893/6647 ≈ 13.4%
//! (most measured blocks live in the test harness, boot init and unreached
//! stdlib, which the scored cost-stage closure does not contain), so
//! measured bias is available for a **minority** of branches. Treating an
//! unmeasured branch as 0.5 would charge the full penalty on ~87% of them,
//! which is (a) not a measurement, (b) arithmetically indistinguishable
//! from raising branch latency by a constant, so the term would rank
//! nothing, and (c) an over-credit of branch *removal* — decision 1609's
//! second clause, and precisely the failure `branch_penalty = 0` existed to
//! refuse.
//!
//! This is **not** in tension with `cost::compose`'s coverage-honesty rule
//! (an uncovered measured hit is charged at the program maximum, never
//! dropped). That rule exists so a transform cannot delete measured *mass*
//! by making a key vanish. An unmeasured branch loses no mass: its block's
//! whole `s(b)` is charged exactly as before, and only the bias-derived
//! **surcharge** is absent. There is nothing to drop, so there is nothing
//! to over-charge.
//!
//! ## The abort-branch finding (plans/M20.md item H asks for this by name)
//!
//! wrela emits an abort branch for every check
//! (`compiler.codegen.naive-locked`), and in any run that does not abort
//! those branches are **always not taken**. Under a bias-derived model an
//! always-not-taken branch has `min(t, n) = 0`, so `u = 0` and the charge
//! is **exactly zero** whatever the swept penalty is; at a residual
//! one-in-a-million abort rate it is 1 cycle of 14. The emit-every-check
//! policy therefore pays no mispredict cost at all on real A76 — and it
//! could not have been argued before item C, because a model with no bias
//! either charged every check branch the full 14 or charged none.
//!
//! This **completes** the half [`super::crosscore`]'s module doc records
//! under "Row 20, abort path": that half showed the abort *branch* is
//! charged while no handler body is, so the policy costs three of the
//! cheapest T1 words per check and one handler for the whole program. The
//! remaining question it left open was whether those branches also cost a
//! mispredict. They do not. Together the two halves are the first
//! quantitative defence of a core doctrine choice this ruler has been able
//! to produce.
//!
//! ## SOG §4.8's front-end terms: what a word-index model can decide
//!
//! `score.rs` scores a **per-fn word stream**. A word's byte offset inside
//! its fn is a codegen fact; the fn's **absolute address** is a layout fact
//! this model does not hold, and `layout.rs` aligns fn entries to 4 bytes
//! only. So the fn's address modulo the 32-byte fetch region is one of 8
//! permitted residues, and every §4.8 rule has to be settled against that
//! residue set. Nothing here assumes address 0.
//!
//! | Row | §4.8 rule | Verdict |
//! | --- | --- | --- |
//! | 23 | > 4 branches per aligned 32 B region | **decided**, ∀ over the 8 permitted residues |
//! | 25 | a ≤ 32 B loop not inside one aligned 32 B region | **decided**, ∀ over the same residues |
//! | 24 | branch and target in different 2 MiB regions | **undecidable**, charges 0 |
//! | 26 | unaligned subroutine entry / branch target | **undecidable**, charges 0 |
//!
//! **Rows 23 and 25 take item I's precedent** (`score::crosses_boundary`
//! quantifies over every permitted `sp` rather than assuming one): compute
//! the charge at each permitted residue and take the **worst**, jointly, so
//! one fn is priced at one placement rather than at a different placement
//! per term. Row 23 is genuinely discriminating under that rule — branches
//! spread more than 4 per 32 B apart cost 0 at *every* residue, while a
//! cluster costs at some residue — which is why it survives the test row 26
//! fails.
//!
//! **Rows 24 and 26 are recorded as undecidable, and charge nothing**,
//! following decision 1615 (item I's identical conflict for a `Cold` base's
//! alignment) rather than inventing an incidence. The ∀ route degenerates
//! for both, and the degeneration is the argument:
//!
//! - Row 26, targets: for any permitted residue exactly **one** residue
//!   class of target offsets is 32 B-aligned, and some residue leaves that
//!   class empty. So "charge if any permitted placement misaligns" charges
//!   **every branch target in the program** — a constant per branch, which
//!   is arithmetically the unconditional branch penalty
//!   `branch_penalty = 0` exists to refuse, and an over-credit of branch
//!   removal (decision 1609's second clause).
//! - Row 26, entries: the same argument gives a constant **per fn**, which
//!   ranks nothing and over-credits fn fusion.
//! - Row 24: the residue set modulo 2 MiB has 524288 members and some
//!   member puts a boundary between any branch and any target at a
//!   different address, so ∀ again charges every branch. (No wrela fn is
//!   remotely close to 2 MiB, so the term's real risk is an *inter*-fn
//!   branch, whose target `score::branch_target_index` cannot resolve at
//!   all.)
//!
//! Decision 1615's three grounds transfer verbatim: it would double-count
//! one uncertainty (the unknown address is already the reason the term is
//! unknown), it would make the term decorative, and for a penalty whose
//! removal is the win a larger value is not automatically the safe one.
//! **The residual risk, stated plainly:** a transformation that worsens
//! branch-target alignment is priced at zero here. The honest fix, if a
//! later rung needs the term real, is the *provable floor* — `#targets −
//! (largest residue class of target offsets)` targets are misaligned at
//! **every** permitted placement, which is a theorem about the code shape
//! rather than an invented incidence — or an emitted fn-entry alignment
//! that makes the residue known.
//!
//! ## The magnitude, and the hole in the profile
//!
//! §4.8 states its four rules and prices **none** of them: no cycle count
//! appears with any of them, so under decision 1602 their magnitudes are T5
//! and cannot be pinned. `bench/a76-pi5.toml` declares exactly one swept
//! front-end magnitude — `[sweep.range_cross_penalty]` (row 27, T4, 1–6,
//! pinned 6) — so that is the dimension rows 23 and 25 are charged at, read
//! through [`SweepPoint`] like every other residual. Sharing it is
//! deliberate: all four §4.8 rules and the range-crossing penalty are
//! instruction-**fetch** effects, and inventing four separate magnitudes the
//! record is silent about is exactly what decision 1602 forbids. Because
//! the magnitude is swept rather than pinned, item J's ∀ sweep makes a
//! candidate whose only gain is padding a dense region win at 1 cycle as
//! well as at 6 — the same guard freeze 1633 puts on barrier removal.
//!
//! **Recorded as an open hole rather than papered over:** row 27 has no
//! *incidence* of its own here, because the profile declares no
//! range-crossing **threshold** row (the plan's prose says "6 cycles for
//! 64–2048 B, 1 cycle at 4096 B"; `[branch]` carries no such row, and
//! inventing one would be a pinned number with no home). Row 27 therefore
//! contributes its swept magnitude and no separate charge. Adding
//! `[branch] range_cross_bytes` is a profile change and belongs to whoever
//! owns `bench/a76-pi5.toml`.
//!
//! ## What the terms reach on the emitted stream (measured, not argued)
//!
//! `unit:branch_terms_census_over_the_cost_corpus`, over all 15 `cost-*`
//! cases. The test prints these live and asserts the structural ones, so
//! the table is a record and not a second source of truth:
//!
//! | Quantity | Count |
//! | --- | --- |
//! | `CostRule::Branch` words | 6211 |
//! | unconditional `B` — perfectly predicted, charged 0 as a **fact** | 484 |
//! | no PC-relative target (`RET` / `BR`) — the return-address stack | 122 |
//! | conditional with two resolvable successors — the biasable set | 5605 |
//! | branches with a bias under `W_flat` | **0** |
//! | §4.8 `dense_excess` / `loop_crossings` | **0 / 0** |
//!
//! Two results worth stating plainly, because both are properties of the
//! spill-everything frame rather than accidents:
//!
//! 1. **Only 5605 of 6211 branch words can ever carry a bias** — the rest
//!    are unconditional `B` or have no PC-relative target. Neither is a
//!    want-of-data case, and the census asserts that any *other*
//!    unresolvable branch (an indirect `BR`, which nobody has priced) fails
//!    closed rather than arriving as a silent zero.
//! 2. **Both §4.8 terms are live but unexercised on this corpus, so the
//!    flat row does not move at all.** The frame is why: every branch is
//!    preceded by the compare, the `cset`, the spill and the reload that
//!    materialised its condition, so five branches never fit inside one
//!    32 B fetch region; and no loop body survives that same frame at 32
//!    bytes or less. The terms are witnessed synthetically, at shapes
//!    `codegen.rs` does not emit — the same outcome item I recorded for
//!    §4.5, and for the same underlying reason.
//!
//!    **This is measured over the whole B-pipe branch set, not just
//!    `CostRule::Branch`.** The density rule counts every branch
//!    instruction the front end predicts — `branch`, `call`, `abort` and
//!    `abort_val`, the four `[latency]` rows on pipeline B — because the
//!    emitted abort pattern (`cmp; b.cond; bl`) clusters exactly those
//!    words, and a `Branch`-only count would score a 6-branch region as 3
//!    and charge nothing. `dense_excess` is 0 over the corpus at the wider
//!    set too, so this term's reach is a fact about the frame rather than
//!    an artefact of which rules were counted.
//!
//! The mispredict term's **maximum** reach is measured too, as an upper
//! bound rather than a prediction: with every block given its own
//! observation at count 1 (so every one of the 73 reads a measured 50/50),
//! the corpus charges **1022** cycles — `cost-runtime` 784, `cost-branchy`
//! 112, `cost-arith` 98, `cost-calls` 28, `cost-bounds-elide` 0,
//! `cost-mem-locality` 0. Real measured bias will be far below that (item C
//! measured 13.4% block coverage), and the two zeros say the term cannot
//! rank those two cases at all.
//!
//! ## Where the §4.8 charge is **not** monotone, and why that is real
//!
//! Item L's monotonicity oracle is worded "a dead instruction never lowers
//! a schedule, footprint cost, or mispredict charge". The **mispredict**
//! half holds here and is asserted
//! (`unit:a_dead_word_never_lowers_the_mispredict_charge`). The **schedule**
//! half does **not** hold once row 23 is live, and the counter-example is
//! not a modelling artefact: inserting a padding word between clustered
//! branches genuinely reduces the number of branches in a 32 B fetch region,
//! which is genuinely cheaper on A76 — that is the entire content of the
//! §4.8 rule. `unit:a_padding_word_can_lower_the_density_charge_and_that_is_real`
//! pins the counter-example so item L cannot land a false oracle silently.

use std::collections::BTreeMap;

use super::rule::{CostRule, EmittedWord};
use super::score::{basic_block_ranges, branch_target_index};
use super::sweep::SweepPoint;
use super::table::CostTable;

/// The sweep dimension every §4.8 front-end incidence is priced at (row
/// 27). Declared once so the sharing is visible at one place rather than
/// spelled at each use.
pub const FRONTEND_SWEEP_DIM: &str = "range_cross_penalty";

/// Permitted alignment of a fn entry, in bytes.
///
/// `layout.rs` places the text section's fns "in its own `BTreeMap` key
/// order, **4-byte aligned**", so a fn's address modulo the 32-byte fetch
/// region is any multiple of 4. This is the step of the residue set rows 23
/// and 25 quantify over — item I's `crosses_boundary` shape, with `sp`'s
/// 16-byte congruence replaced by the text section's 4.
pub const FN_ENTRY_ALIGN_BYTES: u64 = 4;

/// Bytes per emitted word.
const WORD_BYTES: u64 = 4;

/// One measured block observation: how many times the block was entered,
/// and the identity of the **measurement** that says so.
///
/// `source` exists because a count alone cannot say whether two blocks were
/// measured *independently*. Item C's Lane 2 ids are assigned over MWIR
/// blocks and one MWIR block spans a set of emitted-word blocks, so two
/// emitted-word blocks can share a single observation; a ratio built from
/// one datum is an artefact of the attribution, not a measurement. Providers
/// use the identity of the underlying measured block (for item C's bridge:
/// the Lane 2 block index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockObs {
    pub count: u64,
    pub source: u64,
}

impl BlockObs {
    pub fn new(count: u64, source: u64) -> BlockObs {
        BlockObs { count, source }
    }
}

/// Where per-block frequency comes from — the injection point item C wires
/// measured `f` into, shaped exactly like item F's
/// [`super::footprint::HotBlocks`] so the two arrive the same way.
///
/// `Measured` is asked `(fn_key, block_index)`, where `block_index` indexes
/// [`basic_block_ranges`] over that fn's emitted words in layout order —
/// the same identity `HotBlocks` uses and the same one item C's bridge
/// resolves a `<fn_key>#<block_index>` sidecar key to.
#[derive(Clone, Copy)]
pub enum BlockCounts<'a> {
    /// `W_flat` (`f ≡ 1`): **no bias information**, so no mispredict
    /// charge. This variant does not carry counts at all — see the module
    /// doc's point 1 on why that is structural rather than conventional.
    Flat,
    /// Item C's measured `f`: `None` for a block with no measurement.
    Measured(&'a dyn Fn(&str, usize) -> Option<BlockObs>),
}

impl Default for BlockCounts<'_> {
    fn default() -> Self {
        BlockCounts::Flat
    }
}

impl BlockCounts<'_> {
    /// The observation for one emitted-word block, or `None` when there is
    /// none. Under [`BlockCounts::Flat`] this returns `None` **without
    /// reading a count**: `f ≡ 1` is not bias information, and there is
    /// deliberately no path from it to a ratio.
    pub fn obs(&self, fn_key: &str, block_index: usize) -> Option<BlockObs> {
        match self {
            BlockCounts::Flat => None,
            BlockCounts::Measured(f) => f(fn_key, block_index),
        }
    }

    /// Whether any measurement is attached at all. Reported, so a dump or a
    /// unit can say "the flat row charged zero because it had nothing"
    /// rather than "the flat row charged zero".
    pub fn is_measured(&self) -> bool {
        matches!(self, BlockCounts::Measured(_))
    }
}

/// One branch's measured bias: two **independent** observations of its two
/// successors.
///
/// Fields are private and the only constructor is
/// [`BranchBias::from_observations`], so a `BranchBias` cannot be spelled
/// into existence from a single datum or from `f ≡ 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchBias {
    taken: u64,
    not_taken: u64,
}

impl BranchBias {
    /// Build a bias from the two successors' observations, or `None` when
    /// there is no *ratio* to be had:
    ///
    /// - either side unmeasured (no data, no charge);
    /// - both sides resolving to the **same** measurement (one datum is not
    ///   a ratio — see the module doc's point 3);
    /// - both counts zero (nothing was observed).
    pub fn from_observations(taken: Option<BlockObs>, not_taken: Option<BlockObs>) -> Option<Self> {
        let t = taken?;
        let n = not_taken?;
        if t.source == n.source {
            return None;
        }
        if t.count == 0 && n.count == 0 {
            return None;
        }
        Some(BranchBias {
            taken: t.count,
            not_taken: n.count,
        })
    }

    /// Test/oracle constructor: two counts asserted to come from distinct
    /// measurements. Kept explicit so a caller cannot reach it by accident
    /// while wiring a provider.
    pub fn from_distinct_counts(taken: u64, not_taken: u64) -> Option<Self> {
        Self::from_observations(
            Some(BlockObs::new(taken, 0)),
            Some(BlockObs::new(not_taken, 1)),
        )
    }

    pub fn taken(&self) -> u64 {
        self.taken
    }

    pub fn not_taken(&self) -> u64 {
        self.not_taken
    }

    pub fn total(&self) -> u64 {
        self.taken.saturating_add(self.not_taken)
    }

    /// The unpredictability fraction `2·min(t, n) / T` as an exact
    /// `(numerator, denominator)` pair — 0 at either extreme, 1 at 50/50.
    /// Returned as a fraction rather than a float so the charge is integer
    /// arithmetic with a documented rounding direction.
    pub fn unpredictability(&self) -> (u64, u64) {
        (
            2u64.saturating_mul(self.taken.min(self.not_taken)),
            self.total(),
        )
    }
}

/// The per-fn branch terms, keyed by **word index inside the fn**: the
/// bias-derived mispredict inputs and the static §4.8 front-end charges.
///
/// Built once per scored fn, before any block is scheduled, because both
/// halves are properties of the whole word stream rather than of one block
/// slice: §4.8's regions span block boundaries, and a branch's successors
/// are other blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchTerms {
    bias: BTreeMap<usize, BranchBias>,
    frontend: BTreeMap<usize, u64>,
    pub summary: BranchSummary,
}

/// Reported counters — the term is a number, not a boolean, so a collapse
/// to "clean about nothing" is visible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BranchSummary {
    /// `CostRule::Branch` words in the fn.
    pub branches: u64,
    /// Unconditional `B`: one successor, perfectly predicted, charge 0 as a
    /// **fact** rather than for want of data.
    pub unconditional: u64,
    /// Conditional branches whose target or fallthrough is not a resolvable
    /// word in this fn (`BR`, `RET`, an inter-fn branch).
    pub unresolved: u64,
    /// Conditional branches with a real two-sided measured bias.
    pub biased: u64,
    /// Conditional branches with no bias information — the flat row, an
    /// unmeasured block, or two successors sharing one observation.
    pub no_data: u64,
    /// `Σ max(0, branches_in_region − max_branches_per_32b)` at the worst
    /// permitted fn placement (row 23).
    pub dense_excess: u64,
    /// Loops of at most `loop_fit_bytes` whose body spans more than one
    /// aligned fetch region at the worst permitted placement (row 25).
    pub loop_crossings: u64,
    /// The fn-entry residue (bytes modulo the fetch region) the two decided
    /// §4.8 terms were jointly maximised at.
    pub worst_residue: u64,
}

impl BranchTerms {
    /// The flat, measurement-free terms: `BlockCounts::Flat`, so no
    /// mispredict is charged anywhere. §4.8's decided terms still fire —
    /// they are static shape, not frequency.
    pub fn flat(
        fn_key: &str,
        code: &[EmittedWord],
        table: &CostTable,
        point: &SweepPoint,
    ) -> Result<BranchTerms, String> {
        BranchTerms::compute(fn_key, code, table, point, &BlockCounts::Flat)
    }

    pub fn compute(
        fn_key: &str,
        code: &[EmittedWord],
        table: &CostTable,
        point: &SweepPoint,
        counts: &BlockCounts<'_>,
    ) -> Result<BranchTerms, String> {
        let mut out = BranchTerms::default();
        if code.is_empty() {
            return Ok(out);
        }
        out.bias = bias_per_branch(fn_key, code, counts, &mut out.summary)?;
        let fe = frontend_charges(code, table, point, &mut out.summary)?;
        out.frontend = fe;
        Ok(out)
    }

    /// Measured bias for the branch at word `w`, if it has one. `None` is
    /// "no data" and [`branch_mispredict_charge`] turns it into zero.
    pub fn bias_at(&self, w: usize) -> Option<BranchBias> {
        self.bias.get(&w).copied()
    }

    /// The static §4.8 charge attributed to word `w`, in cycles.
    pub fn frontend_at(&self, w: usize) -> u64 {
        self.frontend.get(&w).copied().unwrap_or(0)
    }

    /// Σ of every attributed §4.8 charge. Used by the oracle that the
    /// front-end charge is **exactly** the two decided terms and nothing
    /// else — i.e. that rows 24 and 26 really do charge nothing.
    pub fn total_frontend_charge(&self) -> u64 {
        self.frontend.values().copied().sum()
    }
}

/// The mispredict charge for one branch: `ceil(penalty × 2·min(t,n) / T)`,
/// and **exactly zero** when there is no bias to scale.
///
/// `None` returns before the penalty is read at all — the table's number is
/// not consulted when the model has no data, which is the shape decision
/// 1609 asks for ("no data, no charge") rather than a scaling that happens
/// to land on 0.
pub fn branch_mispredict_charge(penalty: u64, bias: Option<BranchBias>) -> u64 {
    let Some(b) = bias else {
        return 0;
    };
    let (num, den) = b.unpredictability();
    if den == 0 || num == 0 || penalty == 0 {
        return 0;
    }
    // u128 so a measured count near u64::MAX cannot wrap the product;
    // `div_ceil` is decision 1609's rounding direction.
    let scaled = (u128::from(penalty) * u128::from(num)).div_ceil(u128::from(den));
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------
// Bias: the CFG comes from score.rs, never from a second one
// ---------------------------------------------------------------------------

/// True for an unconditional `B` (`0001_01 imm26`). Such a branch has one
/// successor and is perfectly predicted, so it is charged 0 as a fact.
/// `BL` is `CostRule::Call`, not `Branch`, and never reaches here.
fn is_unconditional_b(word: u32) -> bool {
    word & 0xFC00_0000 == 0x1400_0000
}

/// Unconditional branch (register): `1101011 opc op2 op3 Rn op4`. `RET` is
/// `opc = 0010`; `BR` is `0000` and `BLR` is `0001`.
const BR_REG_MASK: u32 = 0xFFFF_FC1F;
const BR_REG_BR: u32 = 0xD61F_0000;
const BR_REG_BLR: u32 = 0xD63F_0000;

/// True for a **computed** branch — `BR` / `BLR`, the register-indirect
/// forms. Nobody has priced one: it has no PC-relative target to compare
/// successors with, and the return-address stack does not predict it the
/// way it predicts `RET`. So it must never arrive as a silent 0.
fn is_indirect_branch_register(word: u32) -> bool {
    let masked = word & BR_REG_MASK;
    masked == BR_REG_BR || masked == BR_REG_BLR
}

/// Word index → block index over [`basic_block_ranges`].
fn block_of(ranges: &[(usize, usize)], n: usize) -> Vec<usize> {
    let mut out = vec![0usize; n];
    for (k, (start, end)) in ranges.iter().enumerate() {
        for w in *start..*end {
            out[w] = k;
        }
    }
    out
}

fn bias_per_branch(
    fn_key: &str,
    code: &[EmittedWord],
    counts: &BlockCounts<'_>,
    summary: &mut BranchSummary,
) -> Result<BTreeMap<usize, BranchBias>, String> {
    let n = code.len();
    // The CFG is score.rs's, not a second one: `basic_block_ranges` already
    // knows the leaders and `branch_target_index` already decodes the
    // PC-relative displacement (plans/M20.md item H says to use them).
    let ranges = basic_block_ranges(code);
    let block = block_of(&ranges, n);
    let mut out = BTreeMap::new();
    for (start, end) in &ranges {
        let _ = start;
        let i = end.saturating_sub(1);
        if code[i].rule != CostRule::Branch {
            continue;
        }
        summary.branches += 1;
        if is_unconditional_b(code[i].word) {
            summary.unconditional += 1;
            continue;
        }
        // **Fail closed on a computed branch.** `RET` charges 0 as a fact
        // — the return-address stack predicts it — and an inter-fn or
        // synthetic word has no successors in this fn to compare. A `BR` /
        // `BLR` is neither: nobody has priced a computed branch, so it must
        // error here rather than arrive as a silent 0 mispredict. This is
        // the model's own guard, not a corpus test's, so it holds on every
        // stream and not only on the cases `cost-*` happens to cover.
        if is_indirect_branch_register(code[i].word) {
            return Err(format!(
                "{fn_key}: word {i} is a computed branch (`BR`/`BLR`, {:#010x}); \
                 no source prices one, so its mispredict charge is undecided",
                code[i].word
            ));
        }
        let fallthrough = i + 1;
        let Some(target) = branch_target_index(code[i].word, i) else {
            // `RET` or an unencoded synthetic word: no target, so no
            // successors to compare.
            summary.unresolved += 1;
            continue;
        };
        if target >= n || fallthrough >= n {
            summary.unresolved += 1;
            continue;
        }
        if block[target] == block[fallthrough] {
            // Both arms land in one block: not two successors, so not a
            // ratio.
            summary.no_data += 1;
            continue;
        }
        match BranchBias::from_observations(
            counts.obs(fn_key, block[target]),
            counts.obs(fn_key, block[fallthrough]),
        ) {
            Some(b) => {
                summary.biased += 1;
                out.insert(i, b);
            }
            None => summary.no_data += 1,
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SOG §4.8: the two decided front-end terms
// ---------------------------------------------------------------------------

/// The aligned fetch-region size §4.8's rules are all stated over.
///
/// The profile spells 32 in **three** rows — `target_align_bytes`,
/// `entry_align_bytes` and `loop_fit_bytes` — and §4.8 means the same
/// aligned 32-byte region in all of them. Reading one and cross-checking
/// the others fails closed if a later profile edit ever splits them, since
/// the model would then not know which one the density rule means.
fn fetch_region_bytes(table: &CostTable) -> Result<u64, String> {
    let row = |k: &str| -> Result<u64, String> {
        table
            .branch_row(k)
            .map(|r| r.value)
            .ok_or_else(|| format!("cost table: [branch.{k}] is required by SOG §4.8's terms"))
    };
    let target = row("target_align_bytes")?;
    let entry = row("entry_align_bytes")?;
    let loop_fit = row("loop_fit_bytes")?;
    if target != entry || entry != loop_fit {
        return Err(format!(
            "cost table: SOG §4.8's fetch region is one size, but [branch] says \
             target_align_bytes={target} entry_align_bytes={entry} loop_fit_bytes={loop_fit}"
        ));
    }
    if target == 0 || target % WORD_BYTES != 0 {
        return Err(format!(
            "cost table: [branch] fetch region {target} is not a positive multiple of {WORD_BYTES}"
        ));
    }
    Ok(target)
}

/// Rows 23 and 25, jointly maximised over the 8 permitted fn-entry
/// residues, then attributed to individual branch words so the charge lands
/// in the `s(b)` of the block that owns the branch.
fn frontend_charges(
    code: &[EmittedWord],
    table: &CostTable,
    point: &SweepPoint,
    summary: &mut BranchSummary,
) -> Result<BTreeMap<usize, u64>, String> {
    let region = fetch_region_bytes(table)?;
    let max_branches = table
        .branch_row("max_branches_per_32b")
        .map(|r| r.value)
        .ok_or_else(|| {
            "cost table: [branch.max_branches_per_32b] is required by SOG §4.8's density rule"
                .to_string()
        })?;
    let loop_fit = region;
    let penalty = point.get(FRONTEND_SWEEP_DIM);

    // §4.8's density rule counts **branch instructions** in a fetch region,
    // and the front end does not care which rule row prices them: a `BL`
    // and the `BL` an abort check emits occupy a predictor slot exactly as
    // a `B.cond` does, and the profile itself puts all four on the B pipe.
    // Counting only `CostRule::Branch` would miss them, and the emitted
    // abort pattern (`cmp; b.cond; bl`) is dense in precisely those words —
    // so a region holding 6 real branches would be scored as 3 and charged
    // nothing.
    let branches: Vec<usize> = (0..code.len())
        .filter(|&i| is_branch_class(code[i].rule))
        .collect();
    // Backward branches: (branch word, target word). §4.8's loop rule only
    // speaks about a loop whose body is at most `loop_fit` bytes; a larger
    // loop cannot fit one region however it is placed, so the rule says
    // nothing about it and nothing is charged.
    //
    // The loop rule reads `CostRule::Branch` only, unlike the density rule
    // above: a loop back-edge is a branch, and a backward `BL` is a
    // recursive call whose body is the callee's, not the words between the
    // call and its target.
    let mut loops: Vec<(usize, usize)> = Vec::new();
    for &i in branches
        .iter()
        .filter(|&&i| code[i].rule == CostRule::Branch)
    {
        if let Some(t) = branch_target_index(code[i].word, i) {
            if t <= i && (i - t + 1) as u64 * WORD_BYTES <= loop_fit {
                loops.push((i, t));
            }
        }
    }

    // ∀ over every permitted fn placement (item I's precedent): score each
    // residue and keep the worst, jointly, so one fn is priced at one
    // address rather than at a different address per term.
    let residues = region / FN_ENTRY_ALIGN_BYTES;
    let mut best: Option<(u64, u64, u64)> = None; // (residue, excess, crossings)
    for step in 0..residues {
        let r = step * FN_ENTRY_ALIGN_BYTES;
        let excess = density_excess(&branches, r, region, max_branches);
        let crossings = loop_crossings(&loops, r, region);
        let total = excess.saturating_add(crossings);
        let better = match best {
            None => true,
            Some((_, e, c)) => total > e.saturating_add(c),
        };
        if better {
            best = Some((r, excess, crossings));
        }
    }
    let (residue, excess, crossings) = best.unwrap_or((0, 0, 0));
    summary.worst_residue = residue;
    summary.dense_excess = excess;
    summary.loop_crossings = crossings;

    // Attribution at that same residue: density excess to the last branch
    // of the offending region, a loop crossing to the loop's own backward
    // branch. Both are branch words, so every §4.8 charge lands on a branch.
    let mut out: BTreeMap<usize, u64> = BTreeMap::new();
    let mut per_region: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for &i in &branches {
        per_region
            .entry(region_of(i, residue, region))
            .or_default()
            .push(i);
    }
    for (_, members) in per_region {
        let over = (members.len() as u64).saturating_sub(max_branches);
        if over > 0 {
            let last = *members.last().expect("non-empty region bucket");
            *out.entry(last).or_insert(0) += over.saturating_mul(penalty);
        }
    }
    for &(i, t) in &loops {
        if region_of(i, residue, region) != region_of(t, residue, region) {
            *out.entry(i).or_insert(0) += penalty;
        }
    }
    Ok(out)
}

/// Aligned fetch region index of word `w` when the fn entry sits at byte
/// residue `r` inside a region.
fn region_of(w: usize, r: u64, region: u64) -> u64 {
    (r + w as u64 * WORD_BYTES) / region
}

/// The rules whose words are **branch instructions** to the front end, and
/// therefore occupy a slot under §4.8's density rule.
///
/// This is the profile's own B-pipe set, not a judgement call: `branch`,
/// `call`, `abort` and `abort_val` are the four `[latency]` rows whose
/// `ports` name pipeline B, and `abort` / `abort_val` are documented there
/// as "branch and link, immed — the abort branch every check emits".
/// `System` (`BRK`) is not a branch and does not predict.
pub fn is_branch_class(rule: CostRule) -> bool {
    matches!(
        rule,
        CostRule::Branch | CostRule::Call | CostRule::Abort | CostRule::AbortVal
    )
}

fn density_excess(branches: &[usize], r: u64, region: u64, max_branches: u64) -> u64 {
    let mut per: BTreeMap<u64, u64> = BTreeMap::new();
    for &i in branches {
        *per.entry(region_of(i, r, region)).or_insert(0) += 1;
    }
    per.values().map(|&c| c.saturating_sub(max_branches)).sum()
}

fn loop_crossings(loops: &[(usize, usize)], r: u64, region: u64) -> u64 {
    loops
        .iter()
        .filter(|(i, t)| region_of(*i, r, region) != region_of(*t, r, region))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::codegen::{CodegenFn, CodegenProgram};
    use crate::cost::footprint::HotBlocks;
    use crate::cost::rule::EmittedWord;
    use crate::cost::score::{CostReport, score_program_at_with_hot};
    use crate::cost::table::{CostTable, load_default};
    use crate::encode;
    use crate::placement::PlacementTable;

    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn placement() -> PlacementTable {
        PlacementTable {
            entries: Vec::new(),
            cores: 0,
        }
    }

    fn point() -> SweepPoint {
        SweepPoint::pinned(&table())
    }

    fn penalty() -> u64 {
        table()
            .branch_row("mispredict_penalty")
            .expect("[branch.mispredict_penalty]")
            .value
    }

    fn frontend_penalty() -> u64 {
        point().get(FRONTEND_SWEEP_DIM)
    }

    fn alu(dst: u8) -> EmittedWord {
        EmittedWord::new(0, String::new(), CostRule::Alu, Some(dst), &[0, 0])
    }

    /// A real `BL` word under one of the three branch-and-link rules
    /// (`call` / `abort` / `abort_val`). The displacement is 0 so it names
    /// itself: `frontend_charges` only reads its position for density.
    fn bl_rule(rule: CostRule) -> EmittedWord {
        EmittedWord::new(encode::enc_bl(0), String::new(), rule, None, &[])
    }

    /// A real `CBZ` word so `branch_target_index` decodes it: `byte_offset`
    /// is relative to the branch word itself.
    fn cbz(byte_offset: i32) -> EmittedWord {
        EmittedWord::new(
            encode::enc_cbz(0, byte_offset, true),
            String::new(),
            CostRule::Branch,
            None,
            &[0],
        )
    }

    fn b(byte_offset: i32) -> EmittedWord {
        EmittedWord::new(
            encode::enc_b(byte_offset),
            String::new(),
            CostRule::Branch,
            None,
            &[],
        )
    }

    fn prog(key: &str, code: Vec<EmittedWord>) -> CodegenProgram {
        let mut fns = BTreeMap::new();
        fns.insert(
            key.to_string(),
            CodegenFn {
                frame_size: 0,
                code,
                relocs: Vec::new(),
            },
        );
        CodegenProgram {
            fns,
            rodata: Vec::new(),
        }
    }

    fn score_flat(p: &CodegenProgram) -> CostReport {
        let t = table();
        score_program_at_with_hot(
            p,
            &t,
            &placement(),
            &SweepPoint::pinned(&t),
            HotBlocks::All,
            BlockCounts::Flat,
        )
        .expect("score")
    }

    fn terms(code: &[EmittedWord], counts: &BlockCounts<'_>) -> BranchTerms {
        let t = table();
        BranchTerms::compute("F.m", code, &t, &SweepPoint::pinned(&t), counts).expect("terms")
    }

    // --- the scaling function ---------------------------------------------

    /// A 99/1 branch is charged ~0 and a 50/50 branch the **full** penalty,
    /// under the same swept `mispredict_penalty`. This is the whole reason
    /// the term may be nonzero at all.
    #[test]
    fn a_99_to_1_branch_is_charged_about_zero_and_a_50_50_the_full_penalty() {
        let p = penalty();
        assert_eq!(p, 14, "the profile pins the pessimistic end of 11-14");
        let biased = BranchBias::from_distinct_counts(99, 1).expect("bias");
        let even = BranchBias::from_distinct_counts(50, 50).expect("bias");
        let lopsided = branch_mispredict_charge(p, Some(biased));
        let coin = branch_mispredict_charge(p, Some(even));
        assert_eq!(coin, p, "a 50/50 branch pays the whole penalty");
        assert!(
            lopsided <= 1,
            "a 99/1 branch must be charged ~0, got {lopsided} of {p}"
        );
        assert!(lopsided < coin);
        // Never taken at all: exactly zero, whatever the penalty is.
        assert_eq!(
            branch_mispredict_charge(p, BranchBias::from_distinct_counts(0, 1000)),
            0
        );
        assert_eq!(
            branch_mispredict_charge(p, BranchBias::from_distinct_counts(1000, 0)),
            0
        );
    }

    /// The charge is monotone in unpredictability and symmetric about 0.5,
    /// so the scaling really is a tent on `min(t, n)/T` and not an accident
    /// of one ratio.
    #[test]
    fn the_charge_is_symmetric_and_monotone_in_unpredictability() {
        let p = penalty();
        let mut prev = 0u64;
        for taken in 0..=50u64 {
            let c =
                branch_mispredict_charge(p, BranchBias::from_distinct_counts(taken, 100 - taken));
            assert!(c >= prev, "charge fell as the branch got less predictable");
            assert_eq!(
                c,
                branch_mispredict_charge(p, BranchBias::from_distinct_counts(100 - taken, taken)),
                "the charge must not depend on which arm is the majority"
            );
            prev = c;
        }
        assert_eq!(prev, p, "the 50/50 end is the full penalty");
    }

    /// An abort branch at one-in-a-million taken is charged ~0 — the finding
    /// plans/M20.md item H asks for by name, and the half
    /// `cost::crosscore`'s row 20 left open.
    #[test]
    fn an_abort_branch_at_one_in_a_million_is_charged_about_zero() {
        let p = penalty();
        let bias = BranchBias::from_distinct_counts(1, 1_000_000).expect("bias");
        let charge = branch_mispredict_charge(p, Some(bias));
        assert!(
            charge <= 1,
            "an always-not-taken check branch must cost ~0 mispredict, got {charge} of {p}"
        );
        // And a genuinely never-taken one is exactly zero at every point of
        // the bracket, which is what makes emit-every-check free here.
        for penalty in [11u64, 14] {
            assert_eq!(
                branch_mispredict_charge(penalty, BranchBias::from_distinct_counts(0, 1_000_000)),
                0
            );
        }
    }

    // --- no data is not a measured 0.5 ------------------------------------

    /// `None` bias charges zero, and it is **structurally** distinct from a
    /// measured 50/50: the flat variant cannot produce a bias at all.
    #[test]
    fn no_data_charges_zero_and_is_structurally_distinct_from_a_measured_half() {
        let p = penalty();
        assert_eq!(branch_mispredict_charge(p, None), 0);
        // A measured 50/50 is the *full* penalty, so the two cases are as
        // far apart as the term can put them.
        assert_eq!(
            branch_mispredict_charge(p, BranchBias::from_distinct_counts(1, 1)),
            p
        );
        // `f ≡ 1` cannot reach the ratio: the flat variant returns `None`
        // for every block without consulting a count.
        let flat = BlockCounts::Flat;
        assert!(!flat.is_measured());
        for block in 0..8 {
            assert!(flat.obs("F.m", block).is_none());
        }
        // Nor can one observation be split into two: same source ⇒ None.
        let one = BlockObs::new(7, 3);
        assert!(BranchBias::from_observations(Some(one), Some(one)).is_none());
        assert!(
            BranchBias::from_observations(Some(BlockObs::new(7, 3)), Some(BlockObs::new(7, 4)))
                .is_some(),
            "two distinct measurements of 7 and 7 are a real 50/50"
        );
        // An unmeasured side is no data, not a half.
        assert!(BranchBias::from_observations(Some(one), None).is_none());
        assert!(BranchBias::from_observations(None, Some(one)).is_none());
    }

    /// The flat row charges **zero mispredict** on a real branchy stream —
    /// the same program scored with a measured 50/50 vector costs strictly
    /// more, so the zero is the absence of data rather than an absence of
    /// the term.
    #[test]
    fn the_flat_row_charges_zero_mispredict() {
        // block 0: cbz over the next word; block 1: the skipped word;
        // block 2: the target.
        let code = vec![cbz(8), alu(1), alu(2)];
        let flat = terms(&code, &BlockCounts::Flat);
        assert_eq!(flat.summary.branches, 1);
        assert_eq!(flat.summary.biased, 0);
        assert_eq!(flat.summary.no_data, 1);
        assert!(flat.bias_at(0).is_none());

        // One shared source would be one datum; give each block its own.
        let per_block = |_: &str, b: usize| Some(BlockObs::new(1, b as u64));
        let measured = terms(&code, &BlockCounts::Measured(&per_block));
        assert_eq!(measured.summary.biased, 1);
        let bias = measured.bias_at(0).expect("measured bias");
        assert_eq!(branch_mispredict_charge(penalty(), Some(bias)), penalty());

        // And at whole-program grain the flat row's total carries no
        // mispredict: adding the penalty to it would change the number.
        let flat_total = score_flat(&prog("F.m", code)).total_proxy_cycles;
        assert!(flat_total > 0);
        assert_eq!(
            flat_total,
            score_flat(&prog("F.m", vec![cbz(8), alu(1), alu(2)])).total_proxy_cycles
        );
    }

    /// An unmeasured branch is the same "no data, no charge" case as the
    /// flat row: with 13.4% block coverage, charging 0.5 would put the full
    /// penalty on ~87% of branches.
    #[test]
    fn an_unmeasured_branch_charges_nothing_at_all() {
        let code = vec![cbz(8), alu(1), alu(2)];
        // Only block 0 measured; the two successors are not.
        let sparse = |_: &str, b: usize| (b == 0).then(|| BlockObs::new(9, 0));
        let t = terms(&code, &BlockCounts::Measured(&sparse));
        assert_eq!(t.summary.biased, 0);
        assert_eq!(t.summary.no_data, 1);
        assert_eq!(branch_mispredict_charge(penalty(), t.bias_at(0)), 0);
    }

    /// An unconditional `B` has one successor and is charged 0 as a **fact**
    /// (perfectly predicted), which the summary keeps distinct from "no
    /// data" so the two reasons never merge.
    #[test]
    fn an_unconditional_branch_is_perfectly_predicted_not_merely_unmeasured() {
        let code = vec![b(8), alu(1), alu(2)];
        let per_block = |_: &str, bi: usize| Some(BlockObs::new(1, bi as u64));
        let t = terms(&code, &BlockCounts::Measured(&per_block));
        assert_eq!(t.summary.branches, 1);
        assert_eq!(t.summary.unconditional, 1);
        assert_eq!(t.summary.biased, 0);
        assert_eq!(t.summary.no_data, 0, "not a want-of-data case");
        assert_eq!(branch_mispredict_charge(penalty(), t.bias_at(0)), 0);
    }

    /// Adding a dead instruction never **lowers** a mispredict charge: a
    /// non-branch word does not change the block partition, so no branch's
    /// bias moves.
    #[test]
    fn a_dead_word_never_lowers_the_mispredict_charge() {
        let code = vec![cbz(8), alu(1), alu(2)];
        let per_block = |_: &str, bi: usize| Some(BlockObs::new(bi as u64 + 1, bi as u64));
        let before = terms(&code, &BlockCounts::Measured(&per_block));
        let mut grown = code.clone();
        // Appended at the end, so no PC-relative displacement shifts.
        grown.push(EmittedWord::new(
            0,
            String::new(),
            CostRule::Alu,
            Some(20),
            &[21, 21],
        ));
        let after = terms(&grown, &BlockCounts::Measured(&per_block));
        let b0 = branch_mispredict_charge(penalty(), before.bias_at(0));
        let a0 = branch_mispredict_charge(penalty(), after.bias_at(0));
        assert!(
            a0 >= b0,
            "a dead word lowered the mispredict charge {b0} -> {a0}"
        );
    }

    // --- SOG §4.8: the decided terms --------------------------------------

    /// Five branches in one aligned 32 B region cost more than four. The
    /// region is 8 words, so five branches inside eight consecutive words
    /// exceed the density rule at some permitted placement while four never
    /// do at any.
    #[test]
    fn five_branches_in_one_32b_region_cost_more_than_four() {
        // `cbz(4)` targets the very next word, so it adds no extra leader
        // beyond the fallthrough and keeps the streams comparable.
        let four = vec![cbz(4), cbz(4), cbz(4), cbz(4), alu(1)];
        let five = vec![cbz(4), cbz(4), cbz(4), cbz(4), cbz(4), alu(1)];
        let t4 = terms(&four, &BlockCounts::Flat);
        let t5 = terms(&five, &BlockCounts::Flat);
        assert_eq!(t4.summary.dense_excess, 0, "four per region is the limit");
        assert_eq!(t5.summary.dense_excess, 1);
        assert_eq!(t4.total_frontend_charge(), 0);
        assert_eq!(t5.total_frontend_charge(), frontend_penalty());
        // And it shows up in the schedule, not only in the summary.
        assert!(
            score_flat(&prog("F.m", five)).total_proxy_cycles
                > score_flat(&prog("F.m", four)).total_proxy_cycles
        );
    }

    /// Spreading branches out beyond four per 32 B costs nothing at **any**
    /// permitted placement — which is what makes row 23 discriminating
    /// rather than a per-branch constant.
    #[test]
    fn branches_more_than_four_per_region_apart_are_never_dense() {
        // Nine words, one branch every other word: at most 4 in any aligned
        // 8-word region.
        let mut code = Vec::new();
        for k in 0..4 {
            code.push(cbz(4));
            code.push(alu(k + 1));
        }
        code.push(alu(9));
        let t = terms(&code, &BlockCounts::Flat);
        assert_eq!(t.summary.dense_excess, 0);
        assert_eq!(t.total_frontend_charge(), 0);
    }

    /// A loop that cannot sit inside one aligned 32 B region costs more than
    /// one that can. A single-word loop fits at every permitted placement; a
    /// 32 B body fits at only one, so ∀ over placements charges it.
    #[test]
    fn a_loop_crossing_the_32b_window_costs_more_than_one_inside_it() {
        // Body = the branch alone (4 B): one region at every placement.
        let inside = vec![alu(1), b(0), alu(2)];
        // Body = 8 words (32 B): fits one region at exactly one of the 8
        // permitted fn placements, so some placement splits it.
        let mut crossing = vec![alu(1)];
        for k in 0..7u8 {
            crossing.push(alu(k + 2));
        }
        crossing.push(cbz(-28)); // back 7 words: body = 8 words = 32 B
        crossing.push(alu(9));
        let ti = terms(&inside, &BlockCounts::Flat);
        let tc = terms(&crossing, &BlockCounts::Flat);
        assert_eq!(ti.summary.loop_crossings, 0, "a 4 B loop always fits");
        assert_eq!(tc.summary.loop_crossings, 1);
        assert_eq!(tc.total_frontend_charge(), frontend_penalty());
        assert!(tc.total_frontend_charge() > ti.total_frontend_charge());
    }

    /// A loop bigger than the fetch region is outside §4.8's rule: it cannot
    /// fit one region however it is placed, so the rule says nothing and
    /// nothing is charged. Recorded because it is the term's boundary.
    #[test]
    fn a_loop_bigger_than_the_region_is_outside_the_rule() {
        let mut code = vec![alu(1)];
        for k in 0..8u8 {
            code.push(alu(k + 2));
        }
        code.push(cbz(-32)); // back 8 words: body = 9 words = 36 B > 32
        let t = terms(&code, &BlockCounts::Flat);
        assert_eq!(t.summary.loop_crossings, 0);
        assert_eq!(t.total_frontend_charge(), 0);
    }

    /// Rows 24 and 26 charge **nothing**, and the oracle is structural: the
    /// whole front-end charge is exactly the two decided terms, so no
    /// undecidable term can have crept in.
    #[test]
    fn the_frontend_charge_is_exactly_the_two_decided_terms() {
        let cases: Vec<Vec<EmittedWord>> = vec![
            vec![cbz(8), alu(1), alu(2)],
            vec![cbz(4), cbz(4), cbz(4), cbz(4), cbz(4), alu(1)],
            vec![alu(1), alu(2), cbz(-4), alu(3)],
            vec![b(8), alu(1), alu(2)],
        ];
        for code in cases {
            let t = terms(&code, &BlockCounts::Flat);
            assert_eq!(
                t.total_frontend_charge(),
                t.summary
                    .dense_excess
                    .saturating_add(t.summary.loop_crossings)
                    .saturating_mul(frontend_penalty()),
                "an undecidable §4.8 term charged something"
            );
        }
    }

    /// Row 26 specifically: an unaligned branch **target** is undecidable,
    /// so two fns whose target offsets differ only in residue class modulo
    /// the fetch region cost the same. Assuming address 0 would separate
    /// them.
    #[test]
    fn unaligned_target_and_entry_are_undecidable_and_charge_nothing() {
        // Target at word 2 (byte 8) — never 32 B-aligned if the fn is.
        let odd = vec![cbz(8), alu(1), alu(2), alu(3)];
        // Same shape, target at word 8 (byte 32) — 32 B-aligned exactly
        // when the fn entry is.
        let mut aligned = vec![cbz(32)];
        for k in 0..8u8 {
            aligned.push(alu(k + 1));
        }
        let a = terms(&odd, &BlockCounts::Flat);
        let b_ = terms(&aligned, &BlockCounts::Flat);
        assert_eq!(a.total_frontend_charge(), 0);
        assert_eq!(b_.total_frontend_charge(), 0);
        // Row 24's threshold is declared and deliberately unused: 2 MiB is
        // not a distance a per-fn word stream can decide.
        assert_eq!(
            table().branch_row("region_bytes").expect("row").value,
            2 << 20
        );
    }

    /// The counter-example item L's monotonicity oracle has to know about: a
    /// padding word can **lower** the density charge, and on a real A76 that
    /// is a real saving rather than a modelling artefact.
    #[test]
    fn a_padding_word_can_lower_the_density_charge_and_that_is_real() {
        // Five branches inside eight words: dense at some placement.
        let dense = vec![
            cbz(4),
            alu(1),
            cbz(4),
            alu(2),
            cbz(4),
            alu(3),
            cbz(4),
            cbz(4),
            alu(4),
        ];
        let before = terms(&dense, &BlockCounts::Flat);
        assert_eq!(before.summary.dense_excess, 1);
        // One padding word between the last two branches spreads them over
        // nine words, so no aligned 8-word region holds five.
        let mut padded = dense.clone();
        padded.insert(7, alu(20));
        let after = terms(&padded, &BlockCounts::Flat);
        assert_eq!(
            after.summary.dense_excess, 0,
            "the padding word is supposed to break the dense region"
        );
        assert!(
            after.total_frontend_charge() < before.total_frontend_charge(),
            "this is the non-monotonicity item L must account for: {} -> {}",
            before.total_frontend_charge(),
            after.total_frontend_charge()
        );
    }

    /// **`BL` and the abort branches count toward §4.8's density.** The
    /// front end predicts them like any other branch, and the emitted abort
    /// pattern (`cmp; b.cond; bl`) is exactly where they cluster. Counting
    /// only `CostRule::Branch` would score this region as three branches
    /// and charge zero where the real one holds six.
    #[test]
    fn call_and_abort_words_count_toward_the_density_rule() {
        // Three `B.cond`s and three abort `BL`s inside eight words: six
        // branch instructions in one aligned 32 B region.
        let code = vec![
            cbz(4),
            bl_rule(CostRule::Abort),
            cbz(4),
            bl_rule(CostRule::AbortVal),
            cbz(4),
            bl_rule(CostRule::Call),
            alu(1),
            alu(2),
        ];
        let t = terms(&code, &BlockCounts::Flat);
        assert_eq!(
            t.summary.dense_excess, 2,
            "six branch instructions in one region is two over the limit of four"
        );
        assert_eq!(t.total_frontend_charge(), 2 * frontend_penalty());

        // The control that isolates the fix: the same three `B.cond`s with
        // the call/abort words replaced by ALU words is under the limit.
        let control = vec![
            cbz(4),
            alu(3),
            cbz(4),
            alu(4),
            cbz(4),
            alu(5),
            alu(1),
            alu(2),
        ];
        assert_eq!(terms(&control, &BlockCounts::Flat).summary.dense_excess, 0);
    }

    /// The density set is the profile's own **B-pipe** set, so a profile
    /// edit that moves a rule onto or off pipeline B cannot leave the rule
    /// silently uncounted.
    #[test]
    fn the_density_set_is_exactly_the_profiles_b_pipe_rules() {
        let t = table();
        for &rule in CostRule::ALL {
            // The cross-core rules are priced by `[crosscore]` and have no
            // `[latency]` row at all; none of them is a branch.
            if rule.is_crosscore() {
                assert!(!is_branch_class(rule), "{rule:?} has no [latency] row");
                continue;
            }
            let row = t
                .latency_row(rule.as_str())
                .unwrap_or_else(|| panic!("[latency.{}] is required", rule.as_str()));
            let on_b = row.ports.split(',').any(|p| p.trim() == "B");
            assert_eq!(
                is_branch_class(rule),
                on_b,
                "{rule:?} is on ports {:?} but is_branch_class says {}",
                row.ports,
                is_branch_class(rule)
            );
        }
    }

    // --- fail-closed ------------------------------------------------------

    /// **A computed branch errors; a `RET` does not.** Nobody has priced a
    /// `BR`/`BLR`, and it has no PC-relative target, so under the old
    /// "no target → unresolved → charge 0" path it would have arrived as a
    /// silent zero on the image surface. The guard is in the model, so it
    /// holds on every stream rather than only on the `cost-*` corpus.
    #[test]
    fn a_computed_branch_fails_closed_and_a_ret_does_not() {
        let t = table();
        let p = point();
        let br = vec![
            alu(1),
            EmittedWord::new(
                encode::enc_br(9),
                String::new(),
                CostRule::Branch,
                None,
                &[9],
            ),
        ];
        let err = BranchTerms::compute("F.m", &br, &t, &p, &BlockCounts::Flat)
            .expect_err("a computed branch must not be priced silently");
        assert!(
            err.contains("computed branch"),
            "the refusal must name what it refused: {err}"
        );

        // `RET` shares the encoding family and stays a fact-charged 0: the
        // return-address stack predicts it.
        let ret = vec![
            alu(1),
            EmittedWord::new(
                encode::enc_ret(30),
                String::new(),
                CostRule::Branch,
                None,
                &[],
            ),
        ];
        let terms = BranchTerms::compute("F.m", &ret, &t, &p, &BlockCounts::Flat).expect("ret");
        assert_eq!(terms.summary.unresolved, 1);
        assert_eq!(terms.summary.biased, 0);
        assert_eq!(branch_mispredict_charge(penalty(), terms.bias_at(1)), 0);
        assert!(
            is_indirect_branch_register(encode::enc_br(9))
                && !is_indirect_branch_register(encode::enc_ret(30)),
            "the decoder must split BR from RET"
        );
    }

    /// The fetch region is read from the profile and cross-checked across
    /// the three rows that spell it, so a profile that splits them cannot
    /// silently pick one.
    #[test]
    fn the_fetch_region_comes_from_the_profile_and_agrees_across_its_rows() {
        let t = table();
        assert_eq!(fetch_region_bytes(&t).expect("region"), 32);
        assert_eq!(t.branch_row("max_branches_per_32b").expect("row").value, 4);
        // Every §4.8 incidence is priced at the one swept front-end
        // magnitude the profile declares, never at a pinned number.
        assert!(
            t.sweep(FRONTEND_SWEEP_DIM).is_some(),
            "[sweep.{FRONTEND_SWEEP_DIM}] must exist for the §4.8 terms to have a magnitude"
        );
        assert_eq!(t.sweep(FRONTEND_SWEEP_DIM).expect("row").pinned, 6);
        assert_eq!(t.sweep(FRONTEND_SWEEP_DIM).expect("row").lo, 1);
    }

    /// The §4.8 magnitude really is swept: at the bracket's low end the
    /// charge shrinks with it, so item J's ∀ moves this term.
    #[test]
    fn the_frontend_magnitude_moves_with_the_sweep() {
        let t = table();
        let code = vec![cbz(4), cbz(4), cbz(4), cbz(4), cbz(4), alu(1)];
        let hi = BranchTerms::compute(
            "F.m",
            &code,
            &t,
            &SweepPoint::pinned(&t).with(FRONTEND_SWEEP_DIM, 6),
            &BlockCounts::Flat,
        )
        .expect("hi");
        let lo = BranchTerms::compute(
            "F.m",
            &code,
            &t,
            &SweepPoint::pinned(&t).with(FRONTEND_SWEEP_DIM, 1),
            &BlockCounts::Flat,
        )
        .expect("lo");
        assert_eq!(hi.total_frontend_charge(), 6);
        assert_eq!(lo.total_frontend_charge(), 1);
    }

    // --- the corpus census ------------------------------------------------

    /// What the two halves of item H actually reach on the emitted stream,
    /// **measured rather than argued** (item I's corpus-census precedent).
    ///
    /// Asserts three things and reports the rest:
    ///
    /// 1. every `CostRule::Branch` word falls into exactly one of the four
    ///    reasons, so no branch is silently uncounted;
    /// 2. under `BlockCounts::Flat` **no** branch anywhere in the corpus has
    ///    a bias — the flat row charges zero mispredict as a measured fact
    ///    over the whole corpus, not only on a synthetic shape;
    /// 3. the whole §4.8 charge is the two decided terms, so no undecidable
    ///    term leaked in on real code.
    ///
    /// Run with `-- --nocapture` for the census itself.
    #[test]
    fn branch_terms_census_over_the_cost_corpus() {
        use crate::cost::stage::codegen_cost_stage_with_placement;
        use crate::opts::win::discover_cost_corpus;

        let t = table();
        let p = SweepPoint::pinned(&t);
        let fe = p.get(FRONTEND_SWEEP_DIM);
        // Hoisted: `penalty()` re-reads and re-parses bench/a76-pi5.toml on
        // every call, and the per-word loop below calls it once per emitted
        // word across the whole corpus — measured at ~40s of a ~42s test.
        let pen = penalty();
        let corpus = discover_cost_corpus();
        assert!(!corpus.is_empty(), "cost corpus empty");

        // A synthetic provider giving every block its own measurement with
        // count 1: every conditional branch reads a measured 50/50, i.e.
        // the **maximum** the mispredict term can ever charge. Not a
        // prediction about `boot-actors` — an upper bound on the term's
        // reach, so a later collapse to "clean about nothing" is visible.
        let per_block = |_: &str, b: usize| Some(BlockObs::new(1, b as u64));

        let mut census = BranchSummary::default();
        let mut charge_flat = 0u64;
        let mut worst_case_mispredict = 0u64;
        let mut cases = 0u64;
        for path in &corpus {
            let case = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            cases += 1;
            let (program, _place) = codegen_cost_stage_with_placement(path)
                .unwrap_or_else(|e| panic!("codegen {case}: {e}"));
            let mut case_max = 0u64;
            for (key, f) in &program.fns {
                let flat = BranchTerms::compute(key, &f.code, &t, &p, &BlockCounts::Flat)
                    .unwrap_or_else(|e| panic!("{case}/{key}: {e}"));
                let s = flat.summary;
                assert_eq!(
                    s.branches,
                    s.unconditional + s.unresolved + s.biased + s.no_data,
                    "{case}/{key}: a branch fell outside every reason"
                );
                assert_eq!(
                    s.biased, 0,
                    "{case}/{key}: the flat row produced a bias — `f ≡ 1` is not bias information"
                );
                assert_eq!(
                    flat.total_frontend_charge(),
                    (s.dense_excess + s.loop_crossings) * fe,
                    "{case}/{key}: an undecidable §4.8 term charged something"
                );
                // The census of *why* each unresolvable branch is charged
                // 0. `BranchTerms::compute` above already fails closed on a
                // computed branch, so reaching here means every one of them
                // is a `RET` or an inter-fn displacement — decoded from the
                // word, never sniffed from the mnemonic text.
                for (i, ew) in f.code.iter().enumerate() {
                    if ew.rule != CostRule::Branch || is_unconditional_b(ew.word) {
                        continue;
                    }
                    assert!(
                        !is_indirect_branch_register(ew.word),
                        "{case}/{key}: word {i} is a computed branch that reached the census"
                    );
                }
                census.branches += s.branches;
                census.unconditional += s.unconditional;
                census.unresolved += s.unresolved;
                census.no_data += s.no_data;
                census.dense_excess += s.dense_excess;
                census.loop_crossings += s.loop_crossings;
                charge_flat += flat.total_frontend_charge();

                let measured =
                    BranchTerms::compute(key, &f.code, &t, &p, &BlockCounts::Measured(&per_block))
                        .unwrap_or_else(|e| panic!("{case}/{key}: {e}"));
                census.biased += measured.summary.biased;
                for w in 0..f.code.len() {
                    let c = branch_mispredict_charge(pen, measured.bias_at(w));
                    case_max += c;
                }
            }
            worst_case_mispredict += case_max;
            println!("  {case}: worst-case mispredict cycles = {case_max}");
        }
        println!(
            "branch census over {cases} cost-* cases: branches={} unconditional={} \
             unresolved={} no_data(flat)={} biased(all-blocks-measured)={} \
             dense_excess={} loop_crossings={} frontend_cycles={} \
             worst_case_mispredict_cycles={}",
            census.branches,
            census.unconditional,
            census.unresolved,
            census.no_data,
            census.biased,
            census.dense_excess,
            census.loop_crossings,
            charge_flat,
            worst_case_mispredict,
        );
        assert!(census.branches > 0, "no branch words in the cost corpus");
    }

    /// The mispredict penalty is swept too: the same measured 50/50 branch
    /// costs 11 at the bracket's low end and 14 at its pinned end.
    #[test]
    fn the_mispredict_penalty_moves_with_the_sweep() {
        let bias = BranchBias::from_distinct_counts(1, 1).expect("bias");
        assert_eq!(branch_mispredict_charge(11, Some(bias)), 11);
        assert_eq!(branch_mispredict_charge(14, Some(bias)), 14);
        let t = table();
        for p in [
            SweepPoint::pinned(&t).with("mispredict_penalty", 11),
            SweepPoint::pinned(&t).with("mispredict_penalty", 14),
        ] {
            assert_eq!(
                branch_mispredict_charge(p.get("mispredict_penalty"), Some(bias)),
                p.get("mispredict_penalty")
            );
        }
    }
}
