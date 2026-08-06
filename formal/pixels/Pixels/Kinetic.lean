import Mathlib

namespace Pixels

theorem kinetic_slack
    (margin perturbation : ℝ)
    (strict : perturbation < margin) :
    0 < margin - perturbation := by
  linarith

end Pixels
