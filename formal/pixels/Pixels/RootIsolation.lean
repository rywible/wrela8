import Mathlib

namespace Pixels

noncomputable def midpoint (lo hi : ℝ) : ℝ := (lo + hi) / 2

theorem bisection_width_halves (lo hi : ℝ) :
    midpoint lo hi - lo = (hi - lo) / 2 ∧
    hi - midpoint lo hi = (hi - lo) / 2 := by
  constructor <;> simp [midpoint] <;> ring

theorem root_bracket_step
    (lo mid hi root : ℝ)
    (hroot : lo ≤ root ∧ root ≤ hi)
    (hleft : root ≤ mid ∨ mid ≤ root) :
    (lo ≤ root ∧ root ≤ mid) ∨ (mid ≤ root ∧ root ≤ hi) := by
  rcases hleft with h | h
  · exact Or.inl ⟨hroot.1, h⟩
  · exact Or.inr ⟨h, hroot.2⟩

theorem monotone_root_bracket_step
    (f : ℝ → ℝ) (lo mid hi root : ℝ)
    (ordered : lo ≤ mid ∧ mid ≤ hi)
    (rootIn : root ∈ Set.Icc lo hi)
    (rootZero : f root = 0)
    (monotone : StrictMonoOn f (Set.Icc lo hi)) :
    (0 ≤ f mid → root ≤ mid) ∧ (f mid < 0 → mid ≤ root) := by
  have midIn : mid ∈ Set.Icc lo hi := ⟨ordered.1, ordered.2⟩
  constructor
  · intro midNonnegative
    by_contra notLeft
    have midBefore : mid < root := lt_of_not_ge notLeft
    have strict := monotone midIn rootIn midBefore
    rw [rootZero] at strict
    linarith
  · intro midNegative
    by_contra notRight
    have rootBefore : root < mid := lt_of_not_ge notRight
    have strict := monotone rootIn midIn rootBefore
    rw [rootZero] at strict
    linarith

structure RootCell where
  lo : ℝ
  hi : ℝ

def RootCell.Contains (cell : RootCell) (x : ℝ) : Prop :=
  x ∈ Set.Icc cell.lo cell.hi

def rootCovered
    (f : ℝ → ℝ) (domain : Set ℝ) (cells : List RootCell) : Prop :=
  ∀ x, x ∈ domain → f x = 0 →
    ∃ cell ∈ cells, x ∈ Set.Icc cell.lo cell.hi

structure RootIsolationState where
  pending : List RootCell
  accepted : List RootCell
  unresolved : Bool

def stateCovers
    (f : ℝ → ℝ) (domain : Set ℝ) (state : RootIsolationState) : Prop :=
  rootCovered f domain (state.accepted ++ state.pending)

/--
Each transition is one concrete bounded-isolator action. A discard carries the
zero-exclusion fact established by interval/Bernstein evaluation; a split
carries the exact midpoint coverage fact; acceptance moves a terminal cell to
the result list; and any exhausted numeric/depth/capacity budget sets the
unresolved latch.
-/
inductive RootIsolationStep (f : ℝ → ℝ) : RootIsolationState → RootIsolationState → Prop
  | discard (cell : RootCell) (rest accepted : List RootCell)
      (rootFree : ∀ x, cell.Contains x → f x ≠ 0) :
      RootIsolationStep f
        ⟨cell :: rest, accepted, false⟩
        ⟨rest, accepted, false⟩
  | accept (cell : RootCell) (rest accepted : List RootCell) :
      RootIsolationStep f
        ⟨cell :: rest, accepted, false⟩
        ⟨rest, cell :: accepted, false⟩
  | split (cell left right : RootCell) (rest accepted : List RootCell)
      (childrenCover : ∀ x, cell.Contains x →
        left.Contains x ∨ right.Contains x) :
      RootIsolationStep f
        ⟨cell :: rest, accepted, false⟩
        ⟨left :: right :: rest, accepted, false⟩
  | fail (pending accepted : List RootCell) :
      RootIsolationStep f
        ⟨pending, accepted, false⟩
        ⟨pending, accepted, true⟩

theorem root_isolation_step_preserves_coverage
    (f : ℝ → ℝ) (domain : Set ℝ) (before after : RootIsolationState)
    (covered : stateCovers f domain before)
    (step : RootIsolationStep f before after) :
    stateCovers f domain after := by
  cases step with
  | discard cell rest accepted rootFree =>
      intro x xDomain xZero
      obtain ⟨candidate, candidateMem, contains⟩ :=
        covered x xDomain xZero
      rcases List.mem_append.mp candidateMem with acceptedMem | pendingMem
      · exact ⟨candidate, List.mem_append.mpr (Or.inl acceptedMem), contains⟩
      · rcases List.mem_cons.mp pendingMem with candidateEq | restMem
        · subst candidate
          exact False.elim (rootFree x contains xZero)
        · exact ⟨candidate, List.mem_append.mpr (Or.inr restMem), contains⟩
  | accept cell rest accepted =>
      intro x xDomain xZero
      obtain ⟨candidate, candidateMem, contains⟩ :=
        covered x xDomain xZero
      rcases List.mem_append.mp candidateMem with acceptedMem | pendingMem
      · exact ⟨candidate,
          List.mem_append.mpr (Or.inl (List.mem_cons.mpr (Or.inr acceptedMem))),
          contains⟩
      · rcases List.mem_cons.mp pendingMem with candidateEq | restMem
        · subst candidate
          exact ⟨cell,
            List.mem_append.mpr (Or.inl (List.mem_cons.mpr (Or.inl rfl))),
            contains⟩
        · exact ⟨candidate, List.mem_append.mpr (Or.inr restMem), contains⟩
  | split cell left right rest accepted childrenCover =>
      intro x xDomain xZero
      obtain ⟨candidate, candidateMem, contains⟩ :=
        covered x xDomain xZero
      rcases List.mem_append.mp candidateMem with acceptedMem | pendingMem
      · exact ⟨candidate, List.mem_append.mpr (Or.inl acceptedMem), contains⟩
      · rcases List.mem_cons.mp pendingMem with candidateEq | restMem
        · subst candidate
          rcases childrenCover x contains with leftContains | rightContains
          · exact ⟨left,
              List.mem_append.mpr
                (Or.inr (List.mem_cons.mpr (Or.inl rfl))),
              leftContains⟩
          · exact ⟨right,
              List.mem_append.mpr
                (Or.inr (List.mem_cons.mpr
                  (Or.inr (List.mem_cons.mpr (Or.inl rfl))))),
              rightContains⟩
        · exact ⟨candidate,
            List.mem_append.mpr
              (Or.inr (List.mem_cons.mpr
                (Or.inr (List.mem_cons.mpr (Or.inr restMem))))),
            contains⟩
  | fail pending accepted =>
      exact covered

theorem root_isolation_trace_preserves_coverage
    (f : ℝ → ℝ) (domain : Set ℝ) (start finish : RootIsolationState)
    (covered : stateCovers f domain start)
    (steps : Relation.ReflTransGen (RootIsolationStep f) start finish) :
    stateCovers f domain finish := by
  induction steps with
  | refl => exact covered
  | tail previous step inductionHypothesis =>
      exact root_isolation_step_preserves_coverage
        f domain _ _ inductionHypothesis step

inductive BoundedRootOutcome where
  | roots (cells : List RootCell)
  | unresolved

def finishRootIsolation
    (capacity : ℕ) (state : RootIsolationState) : BoundedRootOutcome :=
  if state.unresolved = true ∨ state.pending ≠ [] ∨
      capacity < state.accepted.length then
    .unresolved
  else
    .roots state.accepted

theorem bounded_subdivision_complete_or_unresolved
    (f : ℝ → ℝ) (domain : Set ℝ) (capacity : ℕ)
    (start finish : RootIsolationState)
    (initial : stateCovers f domain start)
    (steps : Relation.ReflTransGen (RootIsolationStep f) start finish) :
    finishRootIsolation capacity finish = .unresolved ∨
      ∃ cells,
        finishRootIsolation capacity finish = .roots cells ∧
        cells.length ≤ capacity ∧ rootCovered f domain cells := by
  have preserved :=
    root_isolation_trace_preserves_coverage f domain start finish initial steps
  unfold finishRootIsolation
  split
  · exact Or.inl rfl
  · rename_i ready
    push Not at ready
    right
    refine ⟨finish.accepted, rfl, ready.2.2, ?_⟩
    simpa [stateCovers, ready.2.1] using preserved

theorem bernstein_root_count_zero_excludes
    (coefficient : Fin n → ℝ)
    (strictPositive : ∀ i, 0 < coefficient i)
    (weights : Fin n → ℝ)
    (nonnegative : ∀ i, 0 ≤ weights i)
    (sumOne : ∑ i, weights i = 1) :
    0 < ∑ i, weights i * coefficient i := by
  have each : ∀ i, 0 ≤ weights i * coefficient i :=
    fun i => mul_nonneg (nonnegative i) (le_of_lt (strictPositive i))
  have existsPositive : ∃ i, 0 < weights i := by
    by_contra h
    push Not at h
    have allZero : ∀ i, weights i = 0 :=
      fun i => le_antisymm (h i) (nonnegative i)
    simp [allZero] at sumOne
  rcases existsPositive with ⟨i, hi⟩
  exact Finset.sum_pos' (fun j _ => each j) ⟨i, Finset.mem_univ i,
    mul_pos hi (strictPositive i)⟩

theorem bernstein_identically_zero_is_degenerate
    (coefficient weight : Fin n → ℝ)
    (allZero : ∀ index, coefficient index = 0) :
    ∑ index, weight index * coefficient index = 0 := by
  simp [allZero]

def admissibleSecondaryRoot
    (originFeature feature : Nat) (corridor tHi : ℝ) : Prop :=
  feature ≠ originFeature ∨ corridor < tHi

theorem exact_feature_exclusion_preserves_other_features
    (originFeature feature : Nat) (corridor tHi : ℝ)
    (different : feature ≠ originFeature) :
    admissibleSecondaryRoot originFeature feature corridor tHi := by
  exact Or.inl different

theorem origin_root_beyond_corridor_is_preserved
    (originFeature : Nat) (corridor tHi : ℝ)
    (beyond : corridor < tHi) :
    admissibleSecondaryRoot originFeature originFeature corridor tHi := by
  exact Or.inr beyond

end Pixels
