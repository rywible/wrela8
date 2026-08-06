import Mathlib

namespace Pixels

/--
The scalar Krawczyk map is `x - a*f x`. A continuous self-map of the sealed
correction interval has a fixed point; the strict contraction bound makes it
unique, and nonzero `a` turns a fixed point into a root of `f`.
-/
theorem krawczyk_strict_inclusion
    (f krawczyk : ℝ → ℝ) (a lo hi contraction : ℝ)
    (aNonzero : a ≠ 0)
    (intervalNonempty : lo ≤ hi)
    (definition : ∀ x ∈ Set.Icc lo hi, krawczyk x = x - a * f x)
    (continuous : ContinuousOn krawczyk (Set.Icc lo hi))
    (mapsInside : Set.MapsTo krawczyk (Set.Icc lo hi) (Set.Icc lo hi))
    (contractionStrict : contraction < 1)
    (contracts : ∀ x ∈ Set.Icc lo hi, ∀ y ∈ Set.Icc lo hi,
      |krawczyk x - krawczyk y| ≤ contraction * |x - y|) :
    ∃! x, x ∈ Set.Icc lo hi ∧ f x = 0 := by
  obtain ⟨root, rootIn, rootFixed⟩ :=
    exists_mem_Icc_isFixedPt_of_mapsTo continuous intervalNonempty mapsInside
  have rootZero : f root = 0 := by
    change krawczyk root = root at rootFixed
    rw [definition root rootIn] at rootFixed
    have productZero : a * f root = 0 := by linarith
    exact (mul_eq_zero.mp productZero).resolve_left aNonzero
  refine ⟨root, ⟨rootIn, rootZero⟩, ?_⟩
  intro other otherFacts
  have otherFixed : krawczyk other = other := by
    rw [definition other otherFacts.1, otherFacts.2, mul_zero, sub_zero]
  have bound := contracts root rootIn other otherFacts.1
  rw [rootFixed, otherFixed] at bound
  by_contra different
  have distancePositive : 0 < |root - other| :=
    abs_pos.mpr (sub_ne_zero.mpr (Ne.symm different))
  nlinarith

end Pixels
