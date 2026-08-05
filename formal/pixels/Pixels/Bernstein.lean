import Mathlib

namespace Pixels

/-- A complete Bernstein coefficient sign scan proves the polynomial sign
because evaluation is a convex combination of the coefficients. -/
theorem bernsteinPositiveHull
    {n : Nat} (coeff weight : Fin n → ℝ)
    (hcoeff : ∀ i, 0 < coeff i)
    (hweight : ∀ i, 0 ≤ weight i)
    (hsum : ∑ i, weight i = 1) :
    0 < ∑ i, weight i * coeff i := by
  obtain ⟨i, hi⟩ : ∃ i, 0 < weight i := by
    by_contra h
    push Not at h
    have hz : ∀ i, weight i = 0 := fun j =>
      le_antisymm (h j) (hweight j)
    simp [hz] at hsum
  have hnonnegative : ∀ j, 0 ≤ weight j * coeff j :=
    fun j => mul_nonneg (hweight j) (le_of_lt (hcoeff j))
  exact Finset.sum_pos' (fun j _ => hnonnegative j) ⟨i, Finset.mem_univ i, mul_pos hi (hcoeff i)⟩

theorem bernsteinNegativeHull
    {n : Nat} (coeff weight : Fin n → ℝ)
    (hcoeff : ∀ i, coeff i < 0)
    (hweight : ∀ i, 0 ≤ weight i)
    (hsum : ∑ i, weight i = 1) :
    ∑ i, weight i * coeff i < 0 := by
  have positive := bernsteinPositiveHull (fun i => -coeff i) weight
    (fun i => neg_pos.mpr (hcoeff i)) hweight hsum
  simpa [Finset.mul_sum, mul_comm, mul_left_comm, mul_assoc] using positive

/-- Outward intervals for every coefficient enclose the corresponding
Bernstein evaluation. This is the compiler proof obligation used after the
power-to-Bernstein conversion has rounded each emitted coefficient. -/
theorem bernsteinCoefficientEnclosure
    {n : Nat} (lower exact upper weight : Fin n → ℝ)
    (hlower : ∀ i, lower i ≤ exact i)
    (hupper : ∀ i, exact i ≤ upper i)
    (hweight : ∀ i, 0 ≤ weight i) :
    (∑ i, weight i * lower i) ≤ (∑ i, weight i * exact i) ∧
      (∑ i, weight i * exact i) ≤ (∑ i, weight i * upper i) := by
  constructor
  · exact Finset.sum_le_sum fun i _ =>
      mul_le_mul_of_nonneg_left (hlower i) (hweight i)
  · exact Finset.sum_le_sum fun i _ =>
      mul_le_mul_of_nonneg_left (hupper i) (hweight i)

/-- Canonical power-basis evaluation for every compiler-supported univariate
degree. Degrees zero through five are represented by zero high coefficients. -/
def powerDegreeSix
    (a0 a1 a2 a3 a4 a5 a6 x : ℝ) : ℝ :=
  a0 + a1 * x + a2 * x ^ 2 + a3 * x ^ 3 +
    a4 * x ^ 4 + a5 * x ^ 5 + a6 * x ^ 6

/-- Degree-six Bernstein evaluation in the exact coefficient order emitted by
the compiler: `b0, b1, ..., b6`. -/
def bernsteinDegreeSix
    (b0 b1 b2 b3 b4 b5 b6 x : ℝ) : ℝ :=
  b0 * (1 - x) ^ 6 +
    6 * b1 * x * (1 - x) ^ 5 +
    15 * b2 * x ^ 2 * (1 - x) ^ 4 +
    20 * b3 * x ^ 3 * (1 - x) ^ 3 +
    15 * b4 * x ^ 4 * (1 - x) ^ 2 +
    6 * b5 * x ^ 5 * (1 - x) +
    b6 * x ^ 6

/-- The fixed degree-zero-through-six power-to-Bernstein conversion is exact.
Padding with zero high power coefficients gives each smaller supported degree.
-/
theorem powerToBernsteinDegreeSixExact
    (a0 a1 a2 a3 a4 a5 a6 x : ℝ) :
    powerDegreeSix a0 a1 a2 a3 a4 a5 a6 x =
      bernsteinDegreeSix
        a0
        (a0 + a1 / 6)
        (a0 + a1 / 3 + a2 / 15)
        (a0 + a1 / 2 + a2 / 5 + a3 / 20)
        (a0 + 2 * a1 / 3 + 2 * a2 / 5 + a3 / 5 + a4 / 15)
        (a0 + 5 * a1 / 6 + 2 * a2 / 3 + a3 / 2 + a4 / 3 + a5 / 6)
        (a0 + a1 + a2 + a3 + a4 + a5 + a6)
        x := by
  simp [powerDegreeSix, bernsteinDegreeSix]
  ring

/-- Canonical power-basis evaluation for the maximum supported runtime event
degree. Zero high coefficients represent every smaller fixed degree. -/
def powerDegreeEight
    (a0 a1 a2 a3 a4 a5 a6 a7 a8 x : ℝ) : ℝ :=
  a0 + a1 * x + a2 * x ^ 2 + a3 * x ^ 3 + a4 * x ^ 4 +
    a5 * x ^ 5 + a6 * x ^ 6 + a7 * x ^ 7 + a8 * x ^ 8

/-- Degree-eight Bernstein evaluation in compiler coefficient order. -/
def bernsteinDegreeEight
    (b0 b1 b2 b3 b4 b5 b6 b7 b8 x : ℝ) : ℝ :=
  b0 * (1 - x) ^ 8 +
    8 * b1 * x * (1 - x) ^ 7 +
    28 * b2 * x ^ 2 * (1 - x) ^ 6 +
    56 * b3 * x ^ 3 * (1 - x) ^ 5 +
    70 * b4 * x ^ 4 * (1 - x) ^ 4 +
    56 * b5 * x ^ 5 * (1 - x) ^ 3 +
    28 * b6 * x ^ 6 * (1 - x) ^ 2 +
    8 * b7 * x ^ 7 * (1 - x) +
    b8 * x ^ 8

/-- The runtime's entire fixed degree set, zero through eight, has the exact
power-to-Bernstein conversion used by the reference implementation. -/
theorem powerToBernsteinDegreeEightExact
    (a0 a1 a2 a3 a4 a5 a6 a7 a8 x : ℝ) :
    powerDegreeEight a0 a1 a2 a3 a4 a5 a6 a7 a8 x =
      bernsteinDegreeEight
        a0
        (a0 + a1 / 8)
        (a0 + a1 / 4 + a2 / 28)
        (a0 + 3 * a1 / 8 + 3 * a2 / 28 + a3 / 56)
        (a0 + a1 / 2 + 3 * a2 / 14 + a3 / 14 + a4 / 70)
        (a0 + 5 * a1 / 8 + 5 * a2 / 14 + 5 * a3 / 28 + a4 / 14 + a5 / 56)
        (a0 + 3 * a1 / 4 + 15 * a2 / 28 + 5 * a3 / 14 +
          3 * a4 / 14 + 3 * a5 / 28 + a6 / 28)
        (a0 + 7 * a1 / 8 + 3 * a2 / 4 + 5 * a3 / 8 +
          a4 / 2 + 3 * a5 / 8 + a6 / 4 + a7 / 8)
        (a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8)
        x := by
  simp [powerDegreeEight, bernsteinDegreeEight]
  ring

/-- The compiler's quadratic Taylor radius is derived from the same complete
third-derivative and displacement bounds used by Rust:
`M₃ * δ³ / 3!`. This is the generic theorem instantiated by deformation,
smooth, material, and implicit-sheet remainder records. -/
theorem taylorWithRemainder
    (f : ℝ → ℝ) (x₀ x M₃ δ : ℝ)
    (hsmooth : ContDiffOn ℝ 3 f (Set.uIcc x₀ x))
    (hthird : ∀ y ∈ Set.uIcc x₀ x, |iteratedDeriv 3 f y| ≤ M₃)
    (hdelta : |x - x₀| ≤ δ)
    (hM₃ : 0 ≤ M₃) (hδ : 0 ≤ δ) :
    let model := taylorWithinEval f 2 (Set.uIcc x₀ x) x₀ x
    let radius := M₃ * δ ^ 3 / 6
    model - radius ≤ f x ∧ f x ≤ model + radius := by
  dsimp
  have hradius : 0 ≤ M₃ * δ ^ 3 / 6 := by positivity
  have herror :
      |f x - taylorWithinEval f 2 (Set.uIcc x₀ x) x₀ x| ≤ M₃ * δ ^ 3 / 6 := by
    by_cases hsame : x₀ = x
    · subst x
      simp [hradius]
    · obtain ⟨y, hy, hremainder⟩ :=
        taylor_mean_remainder_lagrange_iteratedDeriv hsame hsmooth
      rw [hremainder, abs_div, abs_mul, abs_pow]
      norm_num
      gcongr
      exact hthird y (Set.uIoo_subset_uIcc_self hy)
  rw [abs_le] at herror
  constructor <;> linarith

/-- A de Casteljau split preserves a strict coefficient sign because every
child coefficient is a convex interpolation of parent coefficients. -/
theorem bernsteinSubdivisionPositive
    (left right t : ℝ) (hleft : 0 < left) (hright : 0 < right)
    (ht0 : 0 ≤ t) (ht1 : t ≤ 1) :
    0 < (1 - t) * left + t * right := by
  have h0 : 0 ≤ 1 - t := sub_nonneg.mpr ht1
  by_cases ht : t = 0
  · simp [ht, hleft]
  · have htpos : 0 < t := lt_of_le_of_ne ht0 (Ne.symm ht)
    positivity

/-- The two correction faces are the same fixed quadratic substitution with
opposite constant corrections. -/
theorem quadraticCorrectionFaces
    (q0 q1 q2 correction x : ℝ) :
    (q0 + q1 * x + q2 * x ^ 2 - correction,
      q0 + q1 * x + q2 * x ^ 2 + correction) =
    (q0 - correction + q1 * x + q2 * x ^ 2,
      q0 + correction + q1 * x + q2 * x ^ 2) := by
  apply Prod.ext <;> simp <;> ring

/-- The derivative program emitted for a power monomial has its canonical
coefficient and degree. -/
theorem powerDerivative (coefficient x : ℝ) (degree : ℕ) :
    HasDerivAt (fun y : ℝ => coefficient * y ^ (degree + 1))
      (coefficient * (degree + 1) * x ^ degree) x := by
  convert (hasDerivAt_pow (degree + 1) x).const_mul coefficient using 1 <;>
    simp [Nat.cast_add, mul_left_comm, mul_comm]

/-- A strict sign scan has zero sign variations, the bounded base case used
before subdivision/root isolation. -/
theorem positiveCoefficientSignVariationZero
    {n : Nat} (coeff : Fin n → ℝ) (h : ∀ i, 0 < coeff i) :
    ¬ ∃ i j, coeff i ≤ 0 ∧ 0 ≤ coeff j := by
  push Not
  intro i j hi
  exact False.elim ((not_le_of_gt (h i)) hi)

end Pixels
