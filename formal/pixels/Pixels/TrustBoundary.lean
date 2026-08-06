import Pixels.RunCertificate
import Pixels.QOrder
import Pixels.FixedQ
import Pixels.Coverage
import Pixels.Compositing
import Pixels.TransparencyTail
import Pixels.DisplayByte
import Pixels.Kinetic
import Pixels.Normal
import Pixels.MaterialBound

namespace Pixels

/--
Derive cell structure from the finite emitted/excluded event census and the
open-run nonzero predicates. In the compiler these hypotheses are respectively
the event-family census, exclusion records, active predicate table, certified
root brackets, and the feature/q-order interpretation rules.
-/
theorem complete_event_cell_preserves_structure
    {Subject State Predicate : Type}
    (emitted excluded changes : Subject → Prop)
    (openRun : Subject → State → State → Prop)
    (active : Subject → Predicate → Prop)
    (value : Predicate → State → ℝ)
    (onOpenRun : Subject → State → State → State → Prop)
    (invariant : OpenRunInvariant State)
    (accounted : ∀ subject, emitted subject ∨ excluded subject)
    (excludedAbsent : ∀ subject, excluded subject → ¬ changes subject)
    (openRunNoActiveZero : ∀ subject before after,
      openRun subject before after →
      ActivePredicatesNonzero active value onOpenRun subject before after)
    (signPersistence : ∀ subject before after,
      openRun subject before after →
      ActivePredicatesNonzero active value onOpenRun subject before after →
      ∀ predicate, active subject predicate →
        SameStrictSign (value predicate before) (value predicate after))
    (identityFromPredicateSigns : ∀ subject before after,
      openRun subject before after →
      (∀ predicate, active subject predicate →
        SameStrictSign (value predicate before) (value predicate after)) →
      invariant.structuralIdentity before after)
    (orderFromPredicateSigns : ∀ subject before after,
      openRun subject before after →
      (∀ predicate, active subject predicate →
        SameStrictSign (value predicate before) (value predicate after)) →
      invariant.qOrder before after)
    (subject : Subject) (before after : State)
    (cell : openRun subject before after) :
    invariant.structuralIdentity before after ∧ invariant.qOrder before after :=
  (conditionalEventCover emitted excluded changes openRun active value onOpenRun
    invariant accounted excludedAbsent openRunNoActiveZero
    signPersistence identityFromPredicateSigns orderFromPredicateSigns).2
      subject before after cell

theorem fixed_q_winner_matches_real_winner
    (realA realB codeA codeB radiusA radiusB : ℝ)
    (aBound : |realA - codeA| ≤ radiusA)
    (bBound : |realB - codeB| ≤ radiusB)
    (separated : codeB + radiusB < codeA - radiusA) :
    realB < realA :=
  fixed_q_winner realA realB codeA codeB radiusA radiusB
    aBound bBound separated

theorem coverage_composite_within_budget
    (alpha exactAlpha front back budget : ℝ)
    (coverageError : |alpha - exactAlpha| ≤ ε)
    (colorBudget : ε * |front - back| ≤ budget) :
    |(alpha * front + (1 - alpha) * back) -
      (exactAlpha * front + (1 - exactAlpha) * back)| ≤ budget :=
  le_trans (coverage_color_error alpha exactAlpha front back coverageError) colorBudget

theorem transparent_summary_within_budget
    (prefixT maximumSuffix budget actualSuffix : ℝ)
    (prefixNonnegative : 0 ≤ prefixT)
    (suffixBound : |actualSuffix| ≤ maximumSuffix)
    (cutoff : prefixT * maximumSuffix ≤ budget) :
    |prefixT * actualSuffix| ≤ budget :=
  transparency_tail prefixT maximumSuffix budget actualSuffix
    prefixNonnegative suffixBound cutoff

theorem display_singleton_is_exact
    (encode : ℝ → Nat) (lo x hi : ℝ)
    (monotone : Monotone encode)
    (contained : lo ≤ x ∧ x ≤ hi)
    (same : encode lo = encode hi) :
    encode x = encode lo :=
  display_singleton encode lo x hi monotone contained same

theorem kinetic_slack_preserves_run
    (margin perturbation : ℝ)
    (strict : perturbation < margin) :
    0 < margin - perturbation :=
  kinetic_slack margin perturbation strict

/--
One indexed bundle of all facts consumed at the renderer trust boundary.
Every proof below is a field emitted by the compiler or checked by the
FrameProgram/runtime verifier; no theorem argument can be borrowed from a
different run, cell, winner, coverage result, or display decision.
-/
structure RendererVerifierFacts where
  Subject : Type
  State : Type
  Predicate : Type
  certificate : AbstractRunCertificate Subject
  residual : Subject → ℝ → ℝ
  runLo : ℝ
  runHi : ℝ
  -- Root/candidate record fields and Tier-0/Tier-1 verifier facts.
  runIntervalNonempty : runLo ≤ runHi
  runContinuous :
    ContinuousOn (residual certificate.winner) (Set.Icc runLo runHi)
  runDerivativeSign :
    StrictMonoOn (residual certificate.winner) (Set.Icc runLo runHi)
  runLowerFace : residual certificate.winner runLo < 0
  runUpperFace : 0 < residual certificate.winner runHi
  featureAccounted :
    ∀ feature, certificate.active feature ∨ certificate.omitted feature
  omittedCannotCross :
    ∀ feature, certificate.omitted feature → ¬certificate.crossing feature
  winnerActive : certificate.active certificate.winner
  rootDefinesCrossing :
    ∀ root, root ∈ Set.Icc runLo runHi →
      residual certificate.winner root = 0 →
      certificate.crossing certificate.winner ∧
        certificate.q certificate.winner = root
  activeCrossingVisible :
    ∀ feature, certificate.active feature →
      certificate.crossing feature → certificate.visible feature
  visibleCrosses :
    ∀ feature, certificate.visible feature → certificate.crossing feature
  strictFeatureOrder :
    ∀ other, other ≠ certificate.winner →
      certificate.active other → certificate.crossing other →
      certificate.q other < certificate.q certificate.winner
  -- Complete event-cell census and structural verifier facts.
  emitted : Subject → Prop
  excluded : Subject → Prop
  changes : Subject → Prop
  openRun : Subject → State → State → Prop
  active : Subject → Predicate → Prop
  value : Predicate → State → ℝ
  onOpenRun : Subject → State → State → State → Prop
  invariant : OpenRunInvariant State
  accounted : ∀ subject, emitted subject ∨ excluded subject
  excludedAbsent : ∀ subject, excluded subject → ¬ changes subject
  openRunNoActiveZero : ∀ subject before after,
    openRun subject before after →
    ActivePredicatesNonzero active value onOpenRun subject before after
  signPersistence : ∀ subject before after,
    openRun subject before after →
    ActivePredicatesNonzero active value onOpenRun subject before after →
    ∀ predicate, active subject predicate →
      SameStrictSign (value predicate before) (value predicate after)
  identityFromPredicateSigns : ∀ subject before after,
    openRun subject before after →
    (∀ predicate, active subject predicate →
      SameStrictSign (value predicate before) (value predicate after)) →
    invariant.structuralIdentity before after
  orderFromPredicateSigns : ∀ subject before after,
    openRun subject before after →
    (∀ predicate, active subject predicate →
      SameStrictSign (value predicate before) (value predicate after)) →
    invariant.qOrder before after
  before : State
  after : State
  cell : openRun certificate.winner before after
  -- Fixed-q record and separation verifier facts.
  competitor : Subject
  codeWinner : ℝ
  codeCompetitor : ℝ
  radiusWinner : ℝ
  radiusCompetitor : ℝ
  winnerBound :
    |certificate.q certificate.winner - codeWinner| ≤ radiusWinner
  competitorBound :
    |certificate.q competitor - codeCompetitor| ≤ radiusCompetitor
  separated : codeCompetitor + radiusCompetitor < codeWinner - radiusWinner
  -- Coverage record and composite-error verifier facts.
  alpha : ℝ
  exactAlpha : ℝ
  front : ℝ
  back : ℝ
  coverageBudget : ℝ
  coverageErrorRadius : ℝ
  coverageError : |alpha - exactAlpha| ≤ coverageErrorRadius
  colorBudget : coverageErrorRadius * |front - back| ≤ coverageBudget
  -- Transfer-summary record and tail verifier facts.
  prefixT : ℝ
  maximumSuffix : ℝ
  transferBudget : ℝ
  actualSuffix : ℝ
  prefixNonnegative : 0 ≤ prefixT
  suffixBound : |actualSuffix| ≤ maximumSuffix
  cutoff : prefixT * maximumSuffix ≤ transferBudget
  -- Display singleton and kinetic-slack verifier facts.
  encode : ℝ → Nat
  displayLo : ℝ
  displayValue : ℝ
  displayHi : ℝ
  displayMonotone : Monotone encode
  displayContained : displayLo ≤ displayValue ∧ displayValue ≤ displayHi
  displaySame : encode displayLo = encode displayHi
  kineticMargin : ℝ
  kineticPerturbation : ℝ
  kineticStrict : kineticPerturbation < kineticMargin

/--
The central theorem consumes only one `RendererVerifierFacts` value, so all
compiler hypotheses are explicitly indexed by and tied to one certified cell.
-/
theorem renderer_trust_boundary (facts : RendererVerifierFacts) :
    ((∃! root,
        root ∈ Set.Icc facts.runLo facts.runHi ∧
          facts.residual facts.certificate.winner root = 0 ∧
          facts.certificate.q facts.certificate.winner = root) ∧
      ExactFirstVisible facts.certificate) ∧
      (facts.invariant.structuralIdentity facts.before facts.after ∧
        facts.invariant.qOrder facts.before facts.after) ∧
      facts.certificate.q facts.competitor <
        facts.certificate.q facts.certificate.winner ∧
      |(facts.alpha * facts.front + (1 - facts.alpha) * facts.back) -
        (facts.exactAlpha * facts.front +
          (1 - facts.exactAlpha) * facts.back)| ≤ facts.coverageBudget ∧
      |facts.prefixT * facts.actualSuffix| ≤ facts.transferBudget ∧
      facts.encode facts.displayValue = facts.encode facts.displayLo ∧
      0 < facts.kineticMargin - facts.kineticPerturbation := by
  exact ⟨
    run_certificate_first_visible facts.certificate facts.residual
      facts.runLo facts.runHi facts.runIntervalNonempty facts.runContinuous
      facts.runDerivativeSign facts.runLowerFace facts.runUpperFace
      facts.featureAccounted facts.omittedCannotCross facts.winnerActive
      facts.rootDefinesCrossing facts.activeCrossingVisible
      facts.visibleCrosses facts.strictFeatureOrder,
    complete_event_cell_preserves_structure facts.emitted facts.excluded
      facts.changes facts.openRun facts.active facts.value facts.onOpenRun
      facts.invariant facts.accounted facts.excludedAbsent
      facts.openRunNoActiveZero facts.signPersistence
      facts.identityFromPredicateSigns facts.orderFromPredicateSigns
      facts.certificate.winner facts.before facts.after facts.cell,
    fixed_q_winner_matches_real_winner
      (facts.certificate.q facts.certificate.winner)
      (facts.certificate.q facts.competitor) facts.codeWinner
      facts.codeCompetitor facts.radiusWinner facts.radiusCompetitor
      facts.winnerBound facts.competitorBound facts.separated,
    coverage_composite_within_budget facts.alpha facts.exactAlpha facts.front
      facts.back facts.coverageBudget facts.coverageError facts.colorBudget,
    transparent_summary_within_budget facts.prefixT facts.maximumSuffix
      facts.transferBudget facts.actualSuffix facts.prefixNonnegative
      facts.suffixBound facts.cutoff,
    display_singleton_is_exact facts.encode facts.displayLo facts.displayValue
      facts.displayHi facts.displayMonotone facts.displayContained
      facts.displaySame,
    kinetic_slack_preserves_run facts.kineticMargin facts.kineticPerturbation
      facts.kineticStrict
  ⟩

end Pixels
