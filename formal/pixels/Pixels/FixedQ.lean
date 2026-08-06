import Mathlib

namespace Pixels

def fixedQ (q dq ddq : Int) : Nat → Int
  | 0 => q
  | n + 1 => fixedQ q dq ddq n + dq + n * ddq

theorem fixed_q_step (q dq ddq : Int) (n : Nat) :
    fixedQ q dq ddq (n + 1) = fixedQ q dq ddq n + dq + n * ddq := by
  rfl

def triangular : Nat → Int
  | 0 => 0
  | n + 1 => triangular n + n

theorem fixed_q_closed_form (q dq ddq : Int) (n : Nat) :
    fixedQ q dq ddq n =
      q + (n : Int) * dq + triangular n * ddq := by
  induction n with
  | zero => simp [fixedQ, triangular]
  | succ n ih =>
      simp [fixedQ, triangular, ih]
      ring

def quantizedInterval (x scale : ℝ) : Set ℝ :=
  Set.Icc ((Int.floor (x / scale) : ℤ) : ℝ)
    ((Int.ceil (x / scale) : ℤ) : ℝ)

theorem outward_quantization_contains
    (x scale : ℝ) :
    x / scale ∈ quantizedInterval x scale := by
  exact ⟨Int.floor_le _, Int.le_ceil _⟩

structure FixedQSetupSound
    (q dq ddq maximumRaw errorRadius : Int) (resetWidth : Nat) : Prop where
  widthPositive : 0 < resetWidth
  widthBound : resetWidth ≤ 64
  recurrenceBound :
    ∀ n, n ≤ resetWidth → |fixedQ q dq ddq n| ≤ maximumRaw
  errorNonnegative : 0 ≤ errorRadius

theorem fixed_q_setup_no_overflow
    (q dq ddq maximumRaw errorRadius : Int) (resetWidth : Nat)
    (widthPositive : 0 < resetWidth)
    (widthBound : resetWidth ≤ 64)
    (coefficientBound : ∀ n, n ≤ resetWidth →
      |q + (n : Int) * dq + triangular n * ddq| ≤ maximumRaw)
    (errorNonnegative : 0 ≤ errorRadius) :
    FixedQSetupSound q dq ddq maximumRaw errorRadius resetWidth := by
  refine {
    widthPositive := widthPositive
    widthBound := widthBound
    errorNonnegative := errorNonnegative
    recurrenceBound := ?_
  }
  intro n hn
  rw [fixed_q_closed_form]
  exact coefficientBound n hn

theorem fixed_q_accumulated_error
    (real code radius stepError : Nat → ℝ)
    (initial : |real 0 - code 0| ≤ radius 0)
    (step : ∀ n,
      |(real (n + 1) - real n) - (code (n + 1) - code n)| ≤ stepError n)
    (radiusStep : ∀ n, radius n + stepError n ≤ radius (n + 1)) :
    ∀ n, |real n - code n| ≤ radius n := by
  intro n
  induction n with
  | zero => exact initial
  | succ n ih =>
      have decomposition :
          real (n + 1) - code (n + 1) =
            (real n - code n) +
              ((real (n + 1) - real n) - (code (n + 1) - code n)) := by
        ring
      rw [decomposition]
      calc
        _ ≤ |real n - code n| +
            |(real (n + 1) - real n) - (code (n + 1) - code n)| :=
          abs_add_le _ _
        _ ≤ radius n + stepError n := add_le_add ih (step n)
        _ ≤ radius (n + 1) := radiusStep n

theorem fixed_q_winner
    (realA realB codeA codeB radiusA radiusB : ℝ)
    (aBound : |realA - codeA| ≤ radiusA)
    (bBound : |realB - codeB| ≤ radiusB)
    (separated : codeB + radiusB < codeA - radiusA) :
    realB < realA := by
  rw [abs_le] at aBound bBound
  linarith

end Pixels
