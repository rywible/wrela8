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

/--
The global shell is deliberately conservative: every nested blend contributes
its `k/4` undershoot. Later compiler passes can split this shell more tightly.
-/
def SmoothObject.supportBudget : SmoothObject → ℚ
  | .leaf _ => 0
  | .blend k left right =>
      left.supportBudget + right.supportBudget + k / 4

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
      nlinarith [ihLeft hleft, ihRight hright]

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
              (left.supportBudget + right.supportBudget) ≤
            min (left.scalar leafValue) (right.scalar leafValue) := by
        apply le_min
        · nlinarith [min_le_left
            (left.leafMin leafValue) (right.leafMin leafValue)]
        · nlinarith [min_le_right
            (left.leafMin leafValue) (right.leafMin leafValue)]
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
    ∀ q id, object.ContainsLeaf id →
      |leafValue id q| ≤ object.supportBudget →
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
  rcases program.object.composedRootHasSupportedLeaf
      (fun id => program.leafValue id q) hvalid hroot
    with ⟨id, hid, hsupported⟩
  exact ⟨id, hid, program.coversSupportedLeaf q id hid hsupported⟩

end Pixels
