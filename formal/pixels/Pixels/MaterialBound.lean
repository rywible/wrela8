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

end Pixels
