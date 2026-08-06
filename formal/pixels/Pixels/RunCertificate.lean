import Pixels.Krawczyk
import Pixels.EventCover

namespace Pixels

/--
The abstract fields shared by the sealed compiler record and the sweep
verifier for one run. `winner` is not an untyped scalar witness: every root,
occupancy, omission, and q-order fact below is indexed by this same feature
type and this same record.
-/
structure AbstractRunCertificate (Feature : Type) where
  winner : Feature
  active : Feature → Prop
  omitted : Feature → Prop
  crossing : Feature → Prop
  visible : Feature → Prop
  q : Feature → ℝ

def ExactFirstVisible {Feature : Type}
    (certificate : AbstractRunCertificate Feature) : Prop :=
  certificate.visible certificate.winner ∧
    ∀ other, certificate.visible other → other ≠ certificate.winner →
      certificate.q other < certificate.q certificate.winner

theorem bernstein_certificate_unique_root
    (residual : ℝ → ℝ) (lo hi : ℝ)
    (ordered : lo ≤ hi)
    (continuous : ContinuousOn residual (Set.Icc lo hi))
    (strictDerivative : StrictMonoOn residual (Set.Icc lo hi))
    (lowerFace : residual lo < 0)
    (upperFace : 0 < residual hi) :
    ∃! root, root ∈ Set.Icc lo hi ∧ residual root = 0 := by
  have zeroIn : (0 : ℝ) ∈ Set.Icc (residual lo) (residual hi) :=
    ⟨lowerFace.le, upperFace.le⟩
  obtain ⟨root, rootIn, rootZero⟩ :=
    intermediate_value_Icc ordered continuous zeroIn
  refine ⟨root, ⟨rootIn, rootZero⟩, ?_⟩
  intro other otherFacts
  exact (strictDerivative.injOn rootIn otherFacts.1
    (rootZero.trans otherFacts.2.symm)).symm

theorem bernstein_certificate_subdivided_unique_root
    (residual : ℝ → ℝ) (lo mid hi : ℝ)
    (ordered : lo ≤ mid ∧ mid ≤ hi)
    (continuous : ContinuousOn residual (Set.Icc lo hi))
    (strictDerivative : StrictMonoOn residual (Set.Icc lo hi))
    (lowerFace : residual lo < 0)
    (upperFace : 0 < residual hi) :
    ∃! root, root ∈ Set.Icc lo hi ∧ residual root = 0 :=
  bernstein_certificate_unique_root residual lo hi
    (ordered.1.trans ordered.2) continuous strictDerivative
    lowerFace upperFace

/--
Tier-0/Tier-1 supplies the face, continuity, and strict derivative facts for
the residual of the *recorded winner*. Event completeness accounts for every
feature as active or omitted; an omitted feature cannot cross. Occupancy
semantics turns the winner's active crossing into visibility, and any visible
competitor is therefore an active crossing governed by the sealed strict
q-order relation.
-/
theorem run_certificate_first_visible
    {Feature : Type}
    (certificate : AbstractRunCertificate Feature)
    (residual : Feature → ℝ → ℝ) (lo hi : ℝ)
    (intervalNonempty : lo ≤ hi)
    (continuous :
      ContinuousOn (residual certificate.winner) (Set.Icc lo hi))
    (derivativeSign :
      StrictMonoOn (residual certificate.winner) (Set.Icc lo hi))
    (lowerFace : residual certificate.winner lo < 0)
    (upperFace : 0 < residual certificate.winner hi)
    (accounted :
      ∀ feature, certificate.active feature ∨ certificate.omitted feature)
    (omittedCannotCross :
      ∀ feature, certificate.omitted feature → ¬certificate.crossing feature)
    (winnerActive : certificate.active certificate.winner)
    (rootDefinesCrossing :
      ∀ root, root ∈ Set.Icc lo hi →
        residual certificate.winner root = 0 →
        certificate.crossing certificate.winner ∧
          certificate.q certificate.winner = root)
    (activeCrossingVisible :
      ∀ feature, certificate.active feature →
        certificate.crossing feature → certificate.visible feature)
    (visibleCrosses :
      ∀ feature, certificate.visible feature → certificate.crossing feature)
    (strictOrder :
      ∀ other, other ≠ certificate.winner →
        certificate.active other → certificate.crossing other →
        certificate.q other < certificate.q certificate.winner) :
    (∃! root,
      root ∈ Set.Icc lo hi ∧
        residual certificate.winner root = 0 ∧
        certificate.q certificate.winner = root) ∧
      ExactFirstVisible certificate := by
  have zeroIn : (0 : ℝ) ∈
      Set.Icc (residual certificate.winner lo)
        (residual certificate.winner hi) :=
    ⟨lowerFace.le, upperFace.le⟩
  obtain ⟨root, rootIn, rootZero⟩ :=
    intermediate_value_Icc intervalNonempty continuous zeroIn
  have crossingAndQ := rootDefinesCrossing root rootIn rootZero
  have uniqueRoot :
      ∃! root,
        root ∈ Set.Icc lo hi ∧
          residual certificate.winner root = 0 ∧
          certificate.q certificate.winner = root := by
    refine ⟨root, ⟨rootIn, rootZero, crossingAndQ.2⟩, ?_⟩
    intro other otherFacts
    exact (derivativeSign.injOn rootIn otherFacts.1
      (rootZero.trans otherFacts.2.1.symm)).symm
  refine ⟨uniqueRoot, ?_⟩
  constructor
  · exact activeCrossingVisible certificate.winner winnerActive crossingAndQ.1
  · intro other otherVisible notWinner
    have otherCrossing := visibleCrosses other otherVisible
    rcases accounted other with otherActive | otherOmitted
    · exact strictOrder other notWinner otherActive otherCrossing
    · exact False.elim (omittedCannotCross other otherOmitted otherCrossing)

inductive CertificateTier
  | bernsteinFaces
  | monotoneTube
  | krawczyk
  deriving DecidableEq

def selectCertificateTier (tier0 tier1 tier2 : Bool) : Option CertificateTier :=
  if tier0 then some .bernsteinFaces
  else if tier1 then some .monotoneTube
  else if tier2 then some .krawczyk
  else none

/-- The executable certificate driver must preserve the sealed Tier 0, Tier 1,
Tier 2 preference order. Ordinary rejection is fallthrough, never tolerance
widening or source rejection. -/
theorem ordered_certificate_selection (tier0 tier1 tier2 : Bool) :
    (tier0 = true →
      selectCertificateTier tier0 tier1 tier2 = some .bernsteinFaces) ∧
    (tier0 = false → tier1 = true →
      selectCertificateTier tier0 tier1 tier2 = some .monotoneTube) ∧
    (tier0 = false → tier1 = false → tier2 = true →
      selectCertificateTier tier0 tier1 tier2 = some .krawczyk) := by
  cases tier0 <;> cases tier1 <;> cases tier2 <;> simp [selectCertificateTier]

end Pixels
