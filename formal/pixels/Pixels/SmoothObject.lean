import Mathlib

namespace Pixels

/-- The polynomial smooth minimum used by the Pixels source contract. -/
def smoothMin (a b k : ℚ) : ℚ :=
  if a ≤ b - k then
    a
  else if b ≤ a - k then
    b
  else
    let h := (1 : ℚ) / 2 + ((1 : ℚ) / 2) * (b - a) / k
    b + (a - b) * h - k * h * (1 - h)

/--
The permanent regression: the composed smooth object has a zero at
`a = b = k/4`, although neither leaf is zero. A candidate generator must
therefore isolate the full composed scalar inside the accumulated support
shell instead of looking only for leaf zeros.
-/
theorem smoothInteriorRoot (k : ℚ) (hk : 0 < k) :
    smoothMin (k / 4) (k / 4) k = 0 := by
  have hleft : ¬k / 4 ≤ k / 4 - k := by linarith
  simp [smoothMin, hleft]
  field_simp
  ring

/-- Both nonzero leaves lie in the `k/4` support shell used to seed isolation. -/
theorem smoothInteriorLeavesInSupport (k : ℚ) (hk : 0 < k) :
    |k / 4| ≤ k / 4 := by
  rw [abs_of_pos (by positivity : 0 < k / 4)]

/-- Outside the active support band the left operand is returned exactly. -/
theorem smoothMinLeftSaturated (a b k : ℚ) (h : a ≤ b - k) :
    smoothMin a b k = a := by
  simp [smoothMin, h]

/--
The support brackets are consequences of the closed smooth-min equation; they
are not hypotheses supplied by a candidate generator.
-/
theorem smoothMinBounds (a b k : ℚ) (hk : 0 < k) :
    min a b - k / 4 ≤ smoothMin a b k ∧
      smoothMin a b k ≤ min a b := by
  unfold smoothMin
  split_ifs with hleft hright
  · have hab : a ≤ b := by linarith
    rw [min_eq_left hab]
    constructor <;> linarith
  · have hba : b ≤ a := by linarith
    rw [min_eq_right hba]
    constructor <;> linarith
  · dsimp
    rcases le_total a b with hab | hba
    · rw [min_eq_left hab]
      constructor <;> field_simp [ne_of_gt hk] <;>
        nlinarith [sq_nonneg (b - a), sq_nonneg (k - (b - a))]
    · rw [min_eq_right hba]
      constructor <;> field_simp [ne_of_gt hk] <;>
        nlinarith [sq_nonneg (a - b), sq_nonneg (k - (a - b))]

/--
Any zero of a smooth composition satisfying the compiler's support bounds is
covered by at least one primitive `k/4` support shell. This is the finite
candidate-generator obligation used by the P-1 walking skeleton.
-/
theorem rootCoveredBySupportBound
    (a b k s : ℚ)
    (hlower : min a b - k / 4 ≤ s)
    (hupper : s ≤ min a b)
    (hz : s = 0) :
    |a| ≤ k / 4 ∨ |b| ≤ k / 4 := by
  have hmin_nonneg : 0 ≤ min a b := by linarith
  have hmin_le : min a b ≤ k / 4 := by linarith
  rcases le_total a b with hab | hba
  · left
    have ha_nonneg : 0 ≤ a := by simpa [min_eq_left hab] using hmin_nonneg
    have ha_le : a ≤ k / 4 := by simpa [min_eq_left hab] using hmin_le
    simpa [abs_of_nonneg ha_nonneg] using ha_le
  · right
    have hb_nonneg : 0 ≤ b := by simpa [min_eq_right hba] using hmin_nonneg
    have hb_le : b ≤ k / 4 := by simpa [min_eq_right hba] using hmin_le
    simpa [abs_of_nonneg hb_nonneg] using hb_le

/-- The permanent interior root is covered by a primitive support candidate. -/
theorem smoothInteriorCandidateCoverage (k : ℚ) (hk : 0 < k) :
    smoothMin (k / 4) (k / 4) k = 0 ∧
      (|k / 4| ≤ k / 4 ∨ |k / 4| ≤ k / 4) := by
  exact ⟨smoothInteriorRoot k hk, Or.inl (smoothInteriorLeavesInSupport k hk)⟩

/-- A closed smooth object tree; leaves are primitive scalar programs. -/
inductive SmoothObject where
  | leaf (id : Nat)
  | blend (k : ℚ) (left right : SmoothObject)
  deriving DecidableEq

def SmoothObject.scalar
    (leafValue : Nat → ℚ) : SmoothObject → ℚ
  | .leaf id => leafValue id
  | .blend k left right =>
      smoothMin (left.scalar leafValue) (right.scalar leafValue) k

def SmoothObject.leafMin
    (leafValue : Nat → ℚ) : SmoothObject → ℚ
  | .leaf id => leafValue id
  | .blend _ left right =>
      min (left.leafMin leafValue) (right.leafMin leafValue)

/-- Every authored blend adds its `k/4` shell to the larger child budget. -/
def SmoothObject.supportBudget : SmoothObject → ℚ
  | .leaf _ => 0
  | .blend k left right =>
      max left.supportBudget right.supportBudget + k / 4

def SmoothObject.WellFormed : SmoothObject → Prop
  | .leaf _ => True
  | .blend k left right =>
      0 < k ∧ left.WellFormed ∧ right.WellFormed

inductive SmoothObject.ContainsLeaf : SmoothObject → Nat → Prop
  | own (id : Nat) : ContainsLeaf (.leaf id) id
  | inLeft {id k left right} :
      ContainsLeaf left id → ContainsLeaf (.blend k left right) id
  | inRight {id k left right} :
      ContainsLeaf right id → ContainsLeaf (.blend k left right) id

/--
An occurrence-level leaf witness carries exactly the sum of `k/4` expansions
on that authored path. This mirrors Rust `LeafSupport.smooth_budget`; it is not
the coarser maximum budget of the whole tree.
-/
inductive SmoothObject.SupportedLeaf : SmoothObject → Nat → ℚ → Prop
  | own (id : Nat) : SupportedLeaf (.leaf id) id 0
  | inLeft {id budget k left right} :
      SupportedLeaf left id budget →
      SupportedLeaf (.blend k left right) id (budget + k / 4)
  | inRight {id budget k left right} :
      SupportedLeaf right id budget →
      SupportedLeaf (.blend k left right) id (budget + k / 4)

theorem SmoothObject.SupportedLeaf.contains
    {object : SmoothObject} {id : Nat} {budget : ℚ}
    (witness : object.SupportedLeaf id budget) :
    object.ContainsLeaf id := by
  induction witness with
  | own id => exact ContainsLeaf.own id
  | inLeft h ih => exact ContainsLeaf.inLeft ih
  | inRight h ih => exact ContainsLeaf.inRight ih

def SmoothObject.supportEnvelope
    (leafValue : Nat → ℚ) : SmoothObject → ℚ
  | .leaf id => leafValue id
  | .blend k left right =>
      min (left.supportEnvelope leafValue - k / 4)
        (right.supportEnvelope leafValue - k / 4)

theorem SmoothObject.supportBudget_nonneg
    (object : SmoothObject) (hvalid : object.WellFormed) :
    0 ≤ object.supportBudget := by
  induction object with
  | leaf id =>
      simp [supportBudget]
  | blend k left right ihLeft ihRight =>
      simp only [WellFormed] at hvalid
      rcases hvalid with ⟨hk, hleft, hright⟩
      simp only [supportBudget]
      nlinarith [ihLeft hleft, ihRight hright,
        le_max_left left.supportBudget right.supportBudget,
        le_max_right left.supportBudget right.supportBudget]

/--
The full composed scalar is enclosed by the minimum primitive scalar and the
sum of the compiler-derived smooth support shells. No bracket is assumed.
-/
theorem SmoothObject.scalarBounds
    (object : SmoothObject)
    (leafValue : Nat → ℚ)
    (hvalid : object.WellFormed) :
    object.leafMin leafValue - object.supportBudget ≤
        object.scalar leafValue ∧
      object.scalar leafValue ≤ object.leafMin leafValue := by
  induction object with
  | leaf id =>
      simp [leafMin, supportBudget, scalar]
  | blend k left right ihLeft ihRight =>
      simp only [WellFormed] at hvalid
      rcases hvalid with ⟨hk, hleft, hright⟩
      have hl := ihLeft hleft
      have hr := ihRight hright
      have hbl := supportBudget_nonneg left hleft
      have hbr := supportBudget_nonneg right hright
      have hs := smoothMinBounds
        (left.scalar leafValue) (right.scalar leafValue) k hk
      have hminLower :
          min (left.leafMin leafValue) (right.leafMin leafValue) -
              max left.supportBudget right.supportBudget ≤
            min (left.scalar leafValue) (right.scalar leafValue) := by
        apply le_min
        · nlinarith [
            min_le_left (left.leafMin leafValue) (right.leafMin leafValue),
            le_max_left left.supportBudget right.supportBudget]
        · nlinarith [
            min_le_right (left.leafMin leafValue) (right.leafMin leafValue),
            le_max_right left.supportBudget right.supportBudget]
      have hminUpper :
          min (left.scalar leafValue) (right.scalar leafValue) ≤
            min (left.leafMin leafValue) (right.leafMin leafValue) :=
        min_le_min hl.2 hr.2
      simp only [leafMin, supportBudget, scalar]
      constructor <;> linarith

theorem SmoothObject.leafMinAttained
    (object : SmoothObject) (leafValue : Nat → ℚ) :
    ∃ id, object.ContainsLeaf id ∧
      leafValue id = object.leafMin leafValue := by
  induction object with
  | leaf id =>
      exact ⟨id, ContainsLeaf.own id, rfl⟩
  | blend k left right ihLeft ihRight =>
      rcases le_total (left.leafMin leafValue) (right.leafMin leafValue)
        with hleft | hright
      · rcases ihLeft with ⟨id, hid, hvalue⟩
        exact ⟨id, ContainsLeaf.inLeft hid,
          hvalue.trans (min_eq_left hleft).symm⟩
      · rcases ihRight with ⟨id, hid, hvalue⟩
        exact ⟨id, ContainsLeaf.inRight hid,
          hvalue.trans (min_eq_right hright).symm⟩

theorem SmoothObject.leafMin_le_of_contains
    (object : SmoothObject) (leafValue : Nat → ℚ) (id : Nat)
    (hcontains : object.ContainsLeaf id) :
    object.leafMin leafValue ≤ leafValue id := by
  induction hcontains with
  | own id => simp [leafMin]
  | inLeft h ih =>
      simp only [leafMin]
      exact le_trans (min_le_left _ _) ih
  | inRight h ih =>
      simp only [leafMin]
      exact le_trans (min_le_right _ _) ih

theorem SmoothObject.supportEnvelope_le_scalar
    (object : SmoothObject)
    (leafValue : Nat → ℚ)
    (hvalid : object.WellFormed) :
    object.supportEnvelope leafValue ≤ object.scalar leafValue := by
  induction object with
  | leaf id =>
      simp [supportEnvelope, scalar]
  | blend k left right ihLeft ihRight =>
      simp only [WellFormed] at hvalid
      rcases hvalid with ⟨hk, hleft, hright⟩
      have hl := ihLeft hleft
      have hr := ihRight hright
      have hs := smoothMinBounds
        (left.scalar leafValue) (right.scalar leafValue) k hk
      simp only [supportEnvelope, scalar]
      have henvelope :
          min (left.supportEnvelope leafValue - k / 4)
              (right.supportEnvelope leafValue - k / 4) =
            min (left.supportEnvelope leafValue)
              (right.supportEnvelope leafValue) - k / 4 := by
        rcases le_total (left.supportEnvelope leafValue)
          (right.supportEnvelope leafValue) with h | h
        · rw [min_eq_left h, min_eq_left (by linarith)]
        · rw [min_eq_right h, min_eq_right (by linarith)]
      have hshift :
          min (left.supportEnvelope leafValue - k / 4)
              (right.supportEnvelope leafValue - k / 4) ≤
            min (left.scalar leafValue) (right.scalar leafValue) - k / 4 := by
        rw [henvelope]
        linarith [min_le_min hl hr]
      exact le_trans hshift hs.1

theorem SmoothObject.supportEnvelopeAttained
    (object : SmoothObject) (leafValue : Nat → ℚ) :
    ∃ id budget, object.SupportedLeaf id budget ∧
      leafValue id - budget = object.supportEnvelope leafValue := by
  induction object with
  | leaf id =>
      exact ⟨id, 0, SupportedLeaf.own id, by simp [supportEnvelope]⟩
  | blend k left right ihLeft ihRight =>
      rcases le_total
          (left.supportEnvelope leafValue - k / 4)
          (right.supportEnvelope leafValue - k / 4)
        with hleft | hright
      · rcases ihLeft with ⟨id, budget, hwitness, hvalue⟩
        refine ⟨id, budget + k / 4, SupportedLeaf.inLeft hwitness, ?_⟩
        change leafValue id - (budget + k / 4) =
          min (left.supportEnvelope leafValue - k / 4)
            (right.supportEnvelope leafValue - k / 4)
        rw [min_eq_left hleft]
        linarith
      · rcases ihRight with ⟨id, budget, hwitness, hvalue⟩
        refine ⟨id, budget + k / 4, SupportedLeaf.inRight hwitness, ?_⟩
        change leafValue id - (budget + k / 4) =
          min (left.supportEnvelope leafValue - k / 4)
            (right.supportEnvelope leafValue - k / 4)
        rw [min_eq_right hright]
        linarith

/--
Every full-composition root has a leaf covered by that occurrence's emitted
path budget, exactly matching the Rust artifact.
-/
theorem SmoothObject.composedRootHasPathSupportedLeaf
    (object : SmoothObject)
    (leafValue : Nat → ℚ)
    (hvalid : object.WellFormed)
    (hroot : object.scalar leafValue = 0) :
    ∃ id budget, object.SupportedLeaf id budget ∧
      |leafValue id| ≤ budget := by
  have henvelope := object.supportEnvelope_le_scalar leafValue hvalid
  have hb := object.scalarBounds leafValue hvalid
  rcases object.supportEnvelopeAttained leafValue
    with ⟨id, budget, hwitness, hvalue⟩
  have hcontains : object.ContainsLeaf id := hwitness.contains
  have hleaf_nonneg : 0 ≤ leafValue id := by
    have hmin : 0 ≤ object.leafMin leafValue := by linarith
    exact le_trans hmin (object.leafMin_le_of_contains leafValue id hcontains)
  have hleaf_le : leafValue id ≤ budget := by
    linarith
  exact ⟨id, budget, hwitness, by simpa [abs_of_nonneg hleaf_nonneg]⟩

/--
Every zero of the full nested composition has a primitive leaf in the derived
global support shell. This is the positive smooth-object completeness result.
-/
theorem SmoothObject.composedRootHasSupportedLeaf
    (object : SmoothObject)
    (leafValue : Nat → ℚ)
    (hvalid : object.WellFormed)
    (hroot : object.scalar leafValue = 0) :
    ∃ id, object.ContainsLeaf id ∧
      |leafValue id| ≤ object.supportBudget := by
  have hb := object.scalarBounds leafValue hvalid
  rcases object.leafMinAttained leafValue with ⟨id, hid, hvalue⟩
  have hnonneg : 0 ≤ object.leafMin leafValue := by
    linarith
  have hbudget : object.leafMin leafValue ≤ object.supportBudget := by
    linarith
  refine ⟨id, hid, ?_⟩
  rw [hvalue]
  simpa [abs_of_nonneg hnonneg] using hbudget

structure QSlab where
  lo : ℚ
  hi : ℚ

def QSlab.Contains (slab : QSlab) (q : ℚ) : Prop :=
  slab.lo ≤ q ∧ q ≤ slab.hi

/--
The compiler artifact links each supported primitive leaf to its q-slab while
`scalar` remains the full composed smooth scalar that root isolation solves.
-/
structure SmoothObjectRootProgram where
  object : SmoothObject
  leafValue : Nat → ℚ → ℚ
  primitiveSlab : Nat → QSlab
  coversSupportedLeaf :
    ∀ q id budget, object.SupportedLeaf id budget →
      |leafValue id q| ≤ budget →
      (primitiveSlab id).Contains q

def SmoothObjectRootProgram.scalar
    (program : SmoothObjectRootProgram) (q : ℚ) : ℚ :=
  program.object.scalar (fun id => program.leafValue id q)

def SmoothObjectRootProgram.HasCandidate
    (program : SmoothObjectRootProgram) (q : ℚ) : Prop :=
  ∃ id, program.object.ContainsLeaf id ∧
    (program.primitiveSlab id).Contains q

/--
A root isolated from the full composed scalar is always covered by a primitive
q-slab candidate emitted by the same root program.
-/
theorem SmoothObjectRootProgram.composedRootHasCandidate
    (program : SmoothObjectRootProgram)
    (hvalid : program.object.WellFormed)
    (q : ℚ)
    (hroot : program.scalar q = 0) :
    program.HasCandidate q := by
  rcases program.object.composedRootHasPathSupportedLeaf
      (fun id => program.leafValue id q) hvalid hroot
    with ⟨id, budget, hid, hsupported⟩
  have hcontains : program.object.ContainsLeaf id := hid.contains
  exact ⟨id, hcontains,
    program.coversSupportedLeaf q id budget hid hsupported⟩

/-
Source-f32 correspondence.

The rational tree above proves the algebraic `k/4` core. The authoritative
source evaluates every blend with rounded f32 operations, so the compiler
adds a nonnegative, versioned per-node rounding slack. This second model makes
that allowance explicit and proves nested-root coverage from it.
-/

inductive RoundedSmoothObject where
  | leaf (id : Nat)
  | blend (k roundingSlack : ℚ)
      (left right : RoundedSmoothObject)

def RoundedSmoothObject.leafMin
    (leafValue : Nat → ℚ) : RoundedSmoothObject → ℚ
  | .leaf id => leafValue id
  | .blend _ _ left right =>
      min (left.leafMin leafValue) (right.leafMin leafValue)

def RoundedSmoothObject.supportBudget : RoundedSmoothObject → ℚ
  | .leaf _ => 0
  | .blend k slack left right =>
      max left.supportBudget right.supportBudget + k / 4 + slack

def RoundedSmoothObject.WellFormed : RoundedSmoothObject → Prop
  | .leaf _ => True
  | .blend k slack left right =>
      0 < k ∧ 0 ≤ slack ∧ left.WellFormed ∧ right.WellFormed

inductive RoundedSmoothObject.Evaluates
    (leafValue : Nat → ℚ) : RoundedSmoothObject → ℚ → Prop
  | leaf (id : Nat) :
      Evaluates leafValue (.leaf id) (leafValue id)
  | blend {k slack left right leftValue rightValue value} :
      Evaluates leafValue left leftValue →
      Evaluates leafValue right rightValue →
      |value - smoothMin leftValue rightValue k| ≤ slack →
      Evaluates leafValue (.blend k slack left right) value

theorem RoundedSmoothObject.supportBudget_nonneg
    (object : RoundedSmoothObject) (hvalid : object.WellFormed) :
    0 ≤ object.supportBudget := by
  induction object with
  | leaf id => simp [supportBudget]
  | blend k slack left right ihLeft ihRight =>
      simp only [WellFormed] at hvalid
      rcases hvalid with ⟨hk, hslack, hleft, hright⟩
      simp only [supportBudget]
      nlinarith [ihLeft hleft, ihRight hright,
        le_max_left left.supportBudget right.supportBudget,
        le_max_right left.supportBudget right.supportBudget]

/--
Every rounded evaluation remains within its compiler-emitted nested support
budget around the minimum primitive scalar.
-/
theorem RoundedSmoothObject.evaluationBounds
    {object : RoundedSmoothObject}
    {leafValue : Nat → ℚ}
    {value : ℚ}
    (hvalid : object.WellFormed)
    (heval : object.Evaluates leafValue value) :
    object.leafMin leafValue - object.supportBudget ≤ value ∧
      value ≤ object.leafMin leafValue + object.supportBudget := by
  induction heval with
  | leaf id =>
      simp [leafMin, supportBudget]
  | @blend k slack left right leftValue rightValue value
      hleftEval hrightEval hround ihLeft ihRight =>
      simp only [WellFormed] at hvalid
      rcases hvalid with ⟨hk, hslack, hleftValid, hrightValid⟩
      have hl := ihLeft hleftValid
      have hr := ihRight hrightValid
      have hbl := supportBudget_nonneg left hleftValid
      have hbr := supportBudget_nonneg right hrightValid
      have hs := smoothMinBounds leftValue rightValue k hk
      have hround' := (abs_le.mp hround)
      have hminLower :
          min (left.leafMin leafValue) (right.leafMin leafValue) -
              max left.supportBudget right.supportBudget ≤
            min leftValue rightValue := by
        apply le_min
        · nlinarith [
            min_le_left (left.leafMin leafValue) (right.leafMin leafValue),
            le_max_left left.supportBudget right.supportBudget]
        · nlinarith [
            min_le_right (left.leafMin leafValue) (right.leafMin leafValue),
            le_max_right left.supportBudget right.supportBudget]
      have hminUpper :
          min leftValue rightValue ≤
            min (left.leafMin leafValue) (right.leafMin leafValue) +
              max left.supportBudget right.supportBudget := by
        rcases le_total (left.leafMin leafValue) (right.leafMin leafValue)
          with hleaf | hleaf
        · calc
            min leftValue rightValue ≤ leftValue := min_le_left _ _
            _ ≤ left.leafMin leafValue + left.supportBudget := hl.2
            _ ≤ min (left.leafMin leafValue) (right.leafMin leafValue) +
                max left.supportBudget right.supportBudget := by
                  rw [min_eq_left hleaf]
                  linarith [le_max_left left.supportBudget right.supportBudget]
        · calc
            min leftValue rightValue ≤ rightValue := min_le_right _ _
            _ ≤ right.leafMin leafValue + right.supportBudget := hr.2
            _ ≤ min (left.leafMin leafValue) (right.leafMin leafValue) +
                max left.supportBudget right.supportBudget := by
                  rw [min_eq_right hleaf]
                  linarith [le_max_right left.supportBudget right.supportBudget]
      simp only [leafMin, supportBudget]
      constructor
      · linarith
      · linarith

theorem RoundedSmoothObject.leafMinAttained
    (object : RoundedSmoothObject) (leafValue : Nat → ℚ) :
    ∃ id, leafValue id = object.leafMin leafValue := by
  induction object with
  | leaf id => exact ⟨id, rfl⟩
  | blend k slack left right ihLeft ihRight =>
      rcases le_total (left.leafMin leafValue) (right.leafMin leafValue)
        with hleft | hright
      · rcases ihLeft with ⟨id, hid⟩
        exact ⟨id, hid.trans (min_eq_left hleft).symm⟩
      · rcases ihRight with ⟨id, hid⟩
        exact ⟨id, hid.trans (min_eq_right hright).symm⟩

/--
The production completeness statement: a zero of a nested source-f32
evaluation has a primitive leaf inside the emitted rounded support shell.
-/
theorem RoundedSmoothObject.rootHasSupportedLeaf
    {object : RoundedSmoothObject}
    {leafValue : Nat → ℚ}
    (hvalid : object.WellFormed)
    (heval : object.Evaluates leafValue 0) :
    ∃ id, |leafValue id| ≤ object.supportBudget := by
  have hb := evaluationBounds hvalid heval
  rcases object.leafMinAttained leafValue with ⟨id, hid⟩
  refine ⟨id, ?_⟩
  rw [hid]
  rw [abs_le]
  constructor <;> linarith

end Pixels
