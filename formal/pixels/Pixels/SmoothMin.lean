import Pixels.SmoothObject

namespace Pixels

/-- Convex gradient weights preserve the larger child derivative bound. -/
theorem convexDerivativeBound
    (da db weight bound : ℝ)
    (hweight : 0 ≤ weight ∧ weight ≤ 1)
    (ha : |da| ≤ bound) (hb : |db| ≤ bound) :
    |weight * da + (1 - weight) * db| ≤ bound := by
  rw [abs_le] at ha hb ⊢
  constructor <;> nlinarith

end Pixels
