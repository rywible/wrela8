import Pixels.Projective

namespace Pixels

inductive PredicateSense where
  | nonnegative
  | nonpositive

def PredicateSense.holds (sense : PredicateSense) (value : ℝ) : Prop :=
  match sense with
  | .nonnegative => 0 ≤ value
  | .nonpositive => value ≤ 0

/-- One emitted finite-feature validity predicate, paired with the positive
homogeneous scale by which the compiler transforms its source-space value. -/
structure ProjectiveValidityPredicate where
  sense : PredicateSense
  sourceValue : ℝ
  homogeneousValue : ℝ
  scale : ℝ

def sourceValidity (predicates : List ProjectiveValidityPredicate) : Prop :=
  ∀ predicate ∈ predicates, predicate.sense.holds predicate.sourceValue

def projectiveValidity (predicates : List ProjectiveValidityPredicate) : Prop :=
  ∀ predicate ∈ predicates, predicate.sense.holds predicate.homogeneousValue

def validityCancellation (predicates : List ProjectiveValidityPredicate) : Prop :=
  ∀ predicate ∈ predicates,
    predicate.homogeneousValue = predicate.scale * predicate.sourceValue ∧
      0 < predicate.scale

theorem predicateSensePositiveScale
    (sense : PredicateSense) (source scale : ℝ) (hscale : 0 < scale) :
    sense.holds source ↔ sense.holds (scale * source) := by
  cases sense <;> simp only [PredicateSense.holds] <;> constructor
  all_goals intro h
  · exact mul_nonneg hscale.le h
  · nlinarith
  · exact mul_nonpos_of_nonneg_of_nonpos hscale.le h
  · nlinarith

/-- Every finite-feature validity predicate has the same truth value before
and after its positive homogeneous scaling. -/
theorem validityPredicatesEquivalent
    (predicates : List ProjectiveValidityPredicate)
    (hcancellation : validityCancellation predicates) :
    sourceValidity predicates ↔ projectiveValidity predicates := by
  constructor
  · intro hsource predicate hmember
    rcases hcancellation predicate hmember with ⟨hequation, hscale⟩
    rw [hequation]
    exact (predicateSensePositiveScale
      predicate.sense predicate.sourceValue predicate.scale hscale).mp
        (hsource predicate hmember)
  · intro hprojective predicate hmember
    rcases hcancellation predicate hmember with ⟨hequation, hscale⟩
    apply (predicateSensePositiveScale
      predicate.sense predicate.sourceValue predicate.scale hscale).mpr
    rw [← hequation]
    exact hprojective predicate hmember

/-- A feature root and all of its emitted validity predicates are preserved
when each is multiplied by its certified positive homogeneous scale. -/
theorem scaledFeatureZeroUnderValidity
    (predicates : List ProjectiveValidityPredicate)
    (sourceValue homogeneousValue scale : ℝ)
    (hcancellation : validityCancellation predicates)
    (hscale : 0 < scale)
    (hequation : homogeneousValue = scale * sourceValue) :
    (sourceValidity predicates ∧ sourceValue = 0) ↔
      (projectiveValidity predicates ∧ homogeneousValue = 0) := by
  rw [validityPredicatesEquivalent predicates hcancellation]
  constructor
  · rintro ⟨hvalid, rfl⟩
    exact ⟨hvalid, by simp [hequation]⟩
  · rintro ⟨hvalid, hzero⟩
    refine ⟨hvalid, ?_⟩
    rw [hequation] at hzero
    exact (mul_eq_zero.mp hzero).resolve_left hscale.ne'

def sphereValue (point center : Vec3) (radius : ℝ) : ℝ :=
  (point.x - center.x) ^ 2 +
    (point.y - center.y) ^ 2 +
    (point.z - center.z) ^ 2 - radius ^ 2

def homogeneousSphere
    (eye ray center : Vec3) (radius q : ℝ) : ℝ :=
  (q * (eye.x - center.x) + ray.x) ^ 2 +
    (q * (eye.y - center.y) + ray.y) ^ 2 +
    (q * (eye.z - center.z) + ray.z) ^ 2 -
    (q * radius) ^ 2

theorem sphereProjectiveEquivalence
    (eye ray center : Vec3) (radius q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * sphereValue (projectivePoint eye ray q) center radius =
      homogeneousSphere eye ray center radius q := by
  simp only [sphereValue, homogeneousSphere, projectivePoint, Vec3.add, Vec3.scale]
  field_simp
  ring

theorem sphereFeatureZeroUnderValidity
    (predicates : List ProjectiveValidityPredicate)
    (eye ray center : Vec3) (radius q : ℝ)
    (hcancellation : validityCancellation predicates) (hq : 0 < q) :
    (sourceValidity predicates ∧
        sphereValue (projectivePoint eye ray q) center radius = 0) ↔
      (projectiveValidity predicates ∧
        homogeneousSphere eye ray center radius q = 0) := by
  apply scaledFeatureZeroUnderValidity predicates
    (sphereValue (projectivePoint eye ray q) center radius)
    (homogeneousSphere eye ray center radius q) (q ^ 2)
    hcancellation (sq_pos_of_pos hq)
  exact (sphereProjectiveEquivalence eye ray center radius q hq.ne').symm

theorem planarFeatureZeroUnderValidity
    (predicates : List ProjectiveValidityPredicate)
    (normal eye ray : Vec3) (offset q : ℝ)
    (hcancellation : validityCancellation predicates) (hq : 0 < q) :
    (sourceValidity predicates ∧
        planeValue normal (projectivePoint eye ray q) offset = 0) ↔
      (projectiveValidity predicates ∧
        homogeneousPlane normal eye ray offset q = 0) := by
  apply scaledFeatureZeroUnderValidity predicates
    (planeValue normal (projectivePoint eye ray q) offset)
    (homogeneousPlane normal eye ray offset q) q hcancellation hq
  exact (projectivePlaneCancellation normal eye ray offset q hq.ne').symm

/-- All emitted planar patches (box faces and cylinder/cone caps) use the
same homogeneous affine cancellation as a source plane. -/
theorem boxFaceProjectiveEquivalence
    (normal eye ray : Vec3) (offset q : ℝ) (hq : q ≠ 0) :
    q * planeValue normal (projectivePoint eye ray q) offset =
      homogeneousPlane normal eye ray offset q :=
  projectivePlaneCancellation normal eye ray offset q hq

theorem roundedBoxFaceProjectiveEquivalence
    (normal eye ray : Vec3) (offset q : ℝ) (hq : q ≠ 0) :
    q * planeValue normal (projectivePoint eye ray q) offset =
      homogeneousPlane normal eye ray offset q :=
  projectivePlaneCancellation normal eye ray offset q hq

theorem cylinderCapProjectiveEquivalence
    (normal eye ray : Vec3) (offset q : ℝ) (hq : q ≠ 0) :
    q * planeValue normal (projectivePoint eye ray q) offset =
      homogeneousPlane normal eye ray offset q :=
  projectivePlaneCancellation normal eye ray offset q hq

theorem coneCapProjectiveEquivalence
    (normal eye ray : Vec3) (offset q : ℝ) (hq : q ≠ 0) :
    q * planeValue normal (projectivePoint eye ray q) offset =
      homogeneousPlane normal eye ray offset q :=
  projectivePlaneCancellation normal eye ray offset q hq

noncomputable def sourceQuadratic (a b c q : ℝ) : ℝ :=
  a / q ^ 2 + b / q + c

def homogeneousQuadratic (a b c q : ℝ) : ℝ :=
  a + b * q + c * q ^ 2

/-- Homogenization used by every quadric feature class. The feature-specific
coefficients are produced by expanding its exact Euclidean implicit equation. -/
theorem genericQuadricProjectiveEquivalence
    (a b c q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * sourceQuadratic a b c q =
      homogeneousQuadratic a b c q := by
  simp only [sourceQuadratic, homogeneousQuadratic]
  field_simp

def segmentSideValue
    (point a b : Vec3) (radiusA radiusB : ℝ) : ℝ :=
  let h : Vec3 := ⟨b.x - a.x, b.y - a.y, b.z - a.z⟩
  let w : Vec3 := ⟨point.x - a.x, point.y - a.y, point.z - a.z⟩
  let h2 := h.dot h
  let axial := w.dot h
  h2 ^ 2 * w.dot w - h2 * axial ^ 2 -
    (radiusA * h2 + (radiusB - radiusA) * axial) ^ 2

def homogeneousSegmentSide
    (eye ray a b : Vec3) (radiusA radiusB q : ℝ) : ℝ :=
  let h : Vec3 := ⟨b.x - a.x, b.y - a.y, b.z - a.z⟩
  let w : Vec3 :=
    ⟨q * (eye.x - a.x) + ray.x,
      q * (eye.y - a.y) + ray.y,
      q * (eye.z - a.z) + ray.z⟩
  let h2 := h.dot h
  let axial := w.dot h
  h2 ^ 2 * w.dot w - h2 * axial ^ 2 -
    (q * (radiusA * h2) + (radiusB - radiusA) * axial) ^ 2

theorem segmentSideProjectiveEquivalence
    (eye ray a b : Vec3) (radiusA radiusB q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * segmentSideValue
      (projectivePoint eye ray q) a b radiusA radiusB =
      homogeneousSegmentSide eye ray a b radiusA radiusB q := by
  simp only [segmentSideValue, homogeneousSegmentSide, projectivePoint,
    Vec3.add, Vec3.scale, Vec3.dot]
  field_simp
  ring

theorem segmentSideFeatureZeroUnderValidity
    (predicates : List ProjectiveValidityPredicate)
    (eye ray a b : Vec3) (radiusA radiusB q : ℝ)
    (hcancellation : validityCancellation predicates) (hq : 0 < q) :
    (sourceValidity predicates ∧
        segmentSideValue (projectivePoint eye ray q) a b radiusA radiusB = 0) ↔
      (projectiveValidity predicates ∧
        homogeneousSegmentSide eye ray a b radiusA radiusB q = 0) := by
  apply scaledFeatureZeroUnderValidity predicates
    (segmentSideValue (projectivePoint eye ray q) a b radiusA radiusB)
    (homogeneousSegmentSide eye ray a b radiusA radiusB q) (q ^ 2)
    hcancellation (sq_pos_of_pos hq)
  exact (segmentSideProjectiveEquivalence
    eye ray a b radiusA radiusB q hq.ne').symm

theorem roundedBoxEdgeProjectiveEquivalence
    (eye ray a b : Vec3) (radius q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * segmentSideValue
      (projectivePoint eye ray q) a b radius radius =
      homogeneousSegmentSide eye ray a b radius radius q :=
  segmentSideProjectiveEquivalence eye ray a b radius radius q hq

theorem roundedBoxCornerProjectiveEquivalence
    (eye ray center : Vec3) (radius q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * sphereValue (projectivePoint eye ray q) center radius =
      homogeneousSphere eye ray center radius q :=
  sphereProjectiveEquivalence eye ray center radius q hq

theorem capsuleSideProjectiveEquivalence
    (eye ray a b : Vec3) (radius q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * segmentSideValue
      (projectivePoint eye ray q) a b radius radius =
      homogeneousSegmentSide eye ray a b radius radius q :=
  segmentSideProjectiveEquivalence eye ray a b radius radius q hq

theorem capsuleCapProjectiveEquivalence
    (eye ray center : Vec3) (radius q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * sphereValue (projectivePoint eye ray q) center radius =
      homogeneousSphere eye ray center radius q :=
  sphereProjectiveEquivalence eye ray center radius q hq

theorem cylinderSideProjectiveEquivalence
    (eye ray a b : Vec3) (radius q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * segmentSideValue
      (projectivePoint eye ray q) a b radius radius =
      homogeneousSegmentSide eye ray a b radius radius q :=
  segmentSideProjectiveEquivalence eye ray a b radius radius q hq

theorem coneSideProjectiveEquivalence
    (eye ray a b : Vec3) (radiusA radiusB q : ℝ) (hq : q ≠ 0) :
    q ^ 2 * segmentSideValue
      (projectivePoint eye ray q) a b radiusA radiusB =
      homogeneousSegmentSide eye ray a b radiusA radiusB q :=
  segmentSideProjectiveEquivalence eye ray a b radiusA radiusB q hq

def torusValue
    (radialSquared axisSquared major minor : ℝ) : ℝ :=
  (radialSquared + axisSquared + major ^ 2 - minor ^ 2) ^ 2 -
    4 * major ^ 2 * radialSquared

def homogeneousTorus
    (radialNumeratorSquared axisNumeratorSquared major minor q : ℝ) : ℝ :=
  (radialNumeratorSquared + axisNumeratorSquared +
      q ^ 2 * (major ^ 2 - minor ^ 2)) ^ 2 -
    4 * major ^ 2 * q ^ 2 * radialNumeratorSquared

theorem torusProjectiveEquivalence
    (radialNumeratorSquared axisNumeratorSquared major minor q : ℝ)
    (hq : q ≠ 0) :
    q ^ 4 *
        torusValue
          (radialNumeratorSquared / q ^ 2)
          (axisNumeratorSquared / q ^ 2)
          major minor =
      homogeneousTorus
        radialNumeratorSquared axisNumeratorSquared major minor q := by
  simp only [torusValue, homogeneousTorus]
  field_simp
  ring

theorem torusFeatureZeroUnderValidity
    (predicates : List ProjectiveValidityPredicate)
    (radialNumeratorSquared axisNumeratorSquared major minor q : ℝ)
    (hcancellation : validityCancellation predicates) (hq : 0 < q) :
    (sourceValidity predicates ∧
        torusValue
          (radialNumeratorSquared / q ^ 2)
          (axisNumeratorSquared / q ^ 2)
          major minor = 0) ↔
      (projectiveValidity predicates ∧
        homogeneousTorus
          radialNumeratorSquared axisNumeratorSquared major minor q = 0) := by
  apply scaledFeatureZeroUnderValidity predicates
    (torusValue
      (radialNumeratorSquared / q ^ 2)
      (axisNumeratorSquared / q ^ 2)
      major minor)
    (homogeneousTorus
      radialNumeratorSquared axisNumeratorSquared major minor q)
    (q ^ 4) hcancellation (pow_pos hq 4)
  exact (torusProjectiveEquivalence
    radialNumeratorSquared axisNumeratorSquared major minor q hq.ne').symm

end Pixels
