import Pixels.Interval

namespace Pixels

theorem material_affine_scalar_contains
    (input scale bias : Interval) (value multiplier offset : ℝ)
    (inputContains : input.Contains value)
    (scaleContains : scale.Contains multiplier)
    (biasContains : bias.Contains offset) :
    ((scale.mulHull input).add bias).Contains
      (multiplier * value + offset) :=
  Interval.affine_contains input scale bias value multiplier offset
    inputContains scaleContains biasContains

theorem material_residual_bound
    (exact proposal residual radius : ℝ)
    (residualBound : |exact - proposal| ≤ residual)
    (roundingBound : 0 ≤ radius) :
    |exact - proposal| ≤ residual + radius := by
  linarith

theorem centered_affine_mean
    (center slope first : ℝ)
    (centeredFirstMoment : first = 0) :
    center + slope * first = center := by
  simp [centeredFirstMoment]

theorem centered_quadratic_mean
    (center linear quadratic first second : ℝ)
    (centeredFirstMoment : first = 0) :
    center + linear * first + quadratic * second =
      center + quadratic * second := by
  simp [centeredFirstMoment]

theorem affine_interval_vector_component
    (center du dv u v radius : ℝ)
    (uBound : |u| ≤ radius)
    (vBound : |v| ≤ radius) :
    |(center + du * u + dv * v) - center| ≤
      |du| * radius + |dv| * radius := by
  calc
    _ = |du * u + dv * v| := by ring_nf
    _ ≤ |du * u| + |dv * v| := abs_add_le _ _
    _ = |du| * |u| + |dv| * |v| := by rw [abs_mul, abs_mul]
    _ ≤ |du| * radius + |dv| * radius := by
      exact add_le_add
        (mul_le_mul_of_nonneg_left uBound (abs_nonneg du))
        (mul_le_mul_of_nonneg_left vBound (abs_nonneg dv))

theorem material_footprint_bounds
    (du dv : ℝ) :
    |du| ≤ max |du| |dv| ∧ |dv| ≤ max |du| |dv| :=
  ⟨le_max_left _ _, le_max_right _ _⟩

theorem moment_variance_nonnegative
    (first second : ℝ) (momentValid : first ^ 2 ≤ second) :
    0 ≤ second - first ^ 2 := by
  linarith

theorem moment_curvature_box_error
    (exact proposal ceiling : ℝ)
    (exactNonnegative : 0 ≤ exact)
    (exactBounded : exact ≤ ceiling)
    (proposalNonnegative : 0 ≤ proposal)
    (proposalBounded : proposal ≤ ceiling) :
    |exact - proposal| ≤ ceiling := by
  rw [abs_le]
  constructor <;> linarith

theorem four_tap_convex_hull_contains
    (lo hi x0 x1 x2 x3 w0 w1 w2 w3 : ℝ)
    (weightSum : w0 + w1 + w2 + w3 = 1)
    (w0Nonnegative : 0 ≤ w0) (w1Nonnegative : 0 ≤ w1)
    (w2Nonnegative : 0 ≤ w2) (w3Nonnegative : 0 ≤ w3)
    (x0Lower : lo ≤ x0) (x1Lower : lo ≤ x1)
    (x2Lower : lo ≤ x2) (x3Lower : lo ≤ x3)
    (x0Upper : x0 ≤ hi) (x1Upper : x1 ≤ hi)
    (x2Upper : x2 ≤ hi) (x3Upper : x3 ≤ hi) :
    lo ≤ w0 * x0 + w1 * x1 + w2 * x2 + w3 * x3 ∧
      w0 * x0 + w1 * x1 + w2 * x2 + w3 * x3 ≤ hi := by
  have lower0 := mul_le_mul_of_nonneg_left x0Lower w0Nonnegative
  have lower1 := mul_le_mul_of_nonneg_left x1Lower w1Nonnegative
  have lower2 := mul_le_mul_of_nonneg_left x2Lower w2Nonnegative
  have lower3 := mul_le_mul_of_nonneg_left x3Lower w3Nonnegative
  have upper0 := mul_le_mul_of_nonneg_left x0Upper w0Nonnegative
  have upper1 := mul_le_mul_of_nonneg_left x1Upper w1Nonnegative
  have upper2 := mul_le_mul_of_nonneg_left x2Upper w2Nonnegative
  have upper3 := mul_le_mul_of_nonneg_left x3Upper w3Nonnegative
  constructor
  · calc
      lo = (w0 + w1 + w2 + w3) * lo := by rw [weightSum]; ring
      _ = w0 * lo + w1 * lo + w2 * lo + w3 * lo := by ring
      _ ≤ w0 * x0 + w1 * x1 + w2 * x2 + w3 * x3 := by
        exact add_le_add (add_le_add (add_le_add lower0 lower1) lower2) lower3
  · calc
      w0 * x0 + w1 * x1 + w2 * x2 + w3 * x3 ≤
          w0 * hi + w1 * hi + w2 * hi + w3 * hi := by
        exact add_le_add (add_le_add (add_le_add upper0 upper1) upper2) upper3
      _ = (w0 + w1 + w2 + w3) * hi := by ring
      _ = hi := by rw [weightSum]; ring

theorem dyadic_cell_budget_sums
    (budget : ℝ) :
    budget / 4 + budget / 4 + budget / 4 + budget / 4 = budget := by
  ring

theorem summary_residual_contains
    (exact candidate residualLo residualHi : ℝ)
    (lo : residualLo ≤ exact - candidate)
    (hi : exact - candidate ≤ residualHi) :
    candidate + residualLo ≤ exact ∧ exact ≤ candidate + residualHi := by
  constructor <;> linarith

end Pixels
