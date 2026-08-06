import Mathlib

namespace Pixels

theorem transparency_tail
    (prefixT maximumSuffix budget actualSuffix : ℝ)
    (prefixNonnegative : 0 ≤ prefixT)
    (suffixBound : |actualSuffix| ≤ maximumSuffix)
    (cutoff : prefixT * maximumSuffix ≤ budget) :
    |prefixT * actualSuffix| ≤ budget := by
  rw [abs_mul, abs_of_nonneg prefixNonnegative]
  exact le_trans (mul_le_mul_of_nonneg_left suffixBound prefixNonnegative) cutoff

end Pixels
