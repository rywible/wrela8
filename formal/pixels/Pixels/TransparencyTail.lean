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

theorem transparency_tail_with_post
    (prefixT maximumSuffix postSensitivity budget actualSuffix : ℝ)
    (prefixNonnegative : 0 ≤ prefixT)
    (postNonnegative : 0 ≤ postSensitivity)
    (suffixBound : |actualSuffix| ≤ maximumSuffix)
    (cutoff : prefixT * maximumSuffix * postSensitivity < budget) :
    |prefixT * actualSuffix| * postSensitivity < budget := by
  rw [abs_mul, abs_of_nonneg prefixNonnegative]
  have attenuated : prefixT * |actualSuffix| ≤ prefixT * maximumSuffix :=
    mul_le_mul_of_nonneg_left suffixBound prefixNonnegative
  exact lt_of_le_of_lt (mul_le_mul_of_nonneg_right attenuated postNonnegative) cutoff

end Pixels
