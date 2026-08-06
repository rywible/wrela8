import Mathlib

namespace Pixels

theorem inverse_depth_normal_orthogonal_u
    (u v q qu qv : ℝ) (nonzero : q ≠ 0) :
    qu * (1 / q - u * qu / (q * q)) +
      qv * (-v * qu / (q * q)) +
      (q - u * qu - v * qv) * (-qu / (q * q)) = 0 := by
  field_simp
  ring

theorem normal_lower_bound_nonzero
    (normSquared lower : ℝ) (lowerPositive : 0 < lower)
    (enclosed : lower ≤ normSquared) :
    normSquared ≠ 0 := by
  linarith

def inverseDepthNormal
    (q qu qv u v : ℝ) : ℝ × ℝ × ℝ :=
  (qu, qv, q - u * qu - v * qv)

theorem inverse_depth_normal_reconstruction
    (q qu qv u v : ℝ) :
    inverseDepthNormal q qu qv u v =
      (qu, qv, q - u * qu - v * qv) := by
  rfl

def normalConeModel
    (q qu qv u v radius : ℝ) :
    (ℝ × ℝ × ℝ) × ℝ :=
  (inverseDepthNormal q qu qv u v, radius)

theorem normal_cone_reconstruction
    (q qu qv u v radius : ℝ) :
    normalConeModel q qu qv u v radius =
      ((qu, qv, q - u * qu - v * qv), radius) := by
  rfl

def normalDot
    (ax ay az bx byy bz : ℝ) : ℝ :=
  ax * bx + ay * byy + az * bz

theorem normal_dot_expansion
    (ax ay az bx byy bz : ℝ) :
    normalDot ax ay az bx byy bz =
      ax * bx + ay * byy + az * bz := by
  rfl

theorem normalized_dot_unit_interval
    (ax ay az bx byy bz denominator : ℝ)
    (denominatorPositive : 0 < denominator)
    (denominatorSquared :
      denominator ^ 2 =
        (ax ^ 2 + ay ^ 2 + az ^ 2) * (bx ^ 2 + byy ^ 2 + bz ^ 2)) :
    -1 ≤ (ax * bx + ay * byy + az * bz) / denominator ∧
      (ax * bx + ay * byy + az * bz) / denominator ≤ 1 := by
  have cauchy :
      (ax * bx + ay * byy + az * bz) ^ 2 ≤ denominator ^ 2 := by
    rw [denominatorSquared]
    nlinarith [sq_nonneg (ax * byy - ay * bx),
      sq_nonneg (ax * bz - az * bx), sq_nonneg (ay * bz - az * byy)]
  constructor
  · apply (le_div_iff₀ denominatorPositive).2
    nlinarith
  · apply (div_le_iff₀ denominatorPositive).2
    nlinarith

theorem normalized_dot_positive_scale
    (dot denominator scale : ℝ)
    (denominatorNonzero : denominator ≠ 0)
    (scalePositive : 0 < scale) :
    (scale ^ 2 * dot) / (scale ^ 2 * denominator) = dot / denominator := by
  field_simp

end Pixels
