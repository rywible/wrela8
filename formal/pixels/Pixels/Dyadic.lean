import Mathlib

namespace Pixels

structure Dyadic where
  mantissa : Int
  exponent : Int

def Dyadic.denote (value : Dyadic) : ℚ :=
  value.mantissa * (2 : ℚ) ^ value.exponent

structure Iv32 where
  lo : Int
  hi : Int
  ordered : lo ≤ hi

def Iv32.denote (exponent : Int) (interval : Iv32) : Set ℚ :=
  { value | interval.lo * (2 : ℚ) ^ exponent ≤ value ∧
            value ≤ interval.hi * (2 : ℚ) ^ exponent }

theorem Iv32.endpoint_containment (interval : Iv32) (exponent : Int) :
    interval.lo * (2 : ℚ) ^ exponent ∈ interval.denote exponent ∧
    interval.hi * (2 : ℚ) ^ exponent ∈ interval.denote exponent := by
  have positive : 0 ≤ (2 : ℚ) ^ exponent :=
    le_of_lt (zpow_pos (by norm_num) exponent)
  constructor
  · exact ⟨le_rfl, mul_le_mul_of_nonneg_right (by exact_mod_cast interval.ordered) positive⟩
  · exact ⟨mul_le_mul_of_nonneg_right (by exact_mod_cast interval.ordered) positive, le_rfl⟩

theorem Iv32.intersection_containment
    (a b : Iv32) (exponent : Int) (x : ℚ)
    (ha : x ∈ a.denote exponent) (hb : x ∈ b.denote exponent) :
    x ∈ a.denote exponent ∩ b.denote exponent :=
  ⟨ha, hb⟩

theorem Iv32.hull_containment
    (a b hull : Iv32) (exponent : Int)
    (ha : a.denote exponent ⊆ hull.denote exponent)
    (hb : b.denote exponent ⊆ hull.denote exponent) :
    a.denote exponent ∪ b.denote exponent ⊆ hull.denote exponent := by
  intro x hx
  cases hx with
  | inl hx => exact ha hx
  | inr hx => exact hb hx

/--
The exact finite source value is enclosed after outward conversion and the
compiler-supplied raw radius is added. The executable Rust/Wrela kernels
establish `lower`/`upper`; this theorem is their real-value contract.
-/
theorem Iv32.f32_conversion_radius_contains
    (source lower upper converted radius : ℚ)
    (rounded : |source - converted| ≤ radius)
    (lowerBound : lower ≤ converted - radius)
    (upperBound : converted + radius ≤ upper) :
    lower ≤ source ∧ source ≤ upper := by
  constructor <;> linarith [abs_le.mp rounded]

theorem Iv32.outward_conversion_contains
    (raw sourceExponent destinationExponent lower upper : Int)
    (lowerBound :
      lower * (2 : ℚ) ^ destinationExponent ≤
        raw * (2 : ℚ) ^ sourceExponent)
    (upperBound :
      raw * (2 : ℚ) ^ sourceExponent ≤
        upper * (2 : ℚ) ^ destinationExponent) :
    raw * (2 : ℚ) ^ sourceExponent ∈
      (Iv32.mk lower upper (by
        have positive : 0 < (2 : ℚ) ^ destinationExponent :=
          zpow_pos (by norm_num) destinationExponent
        have scaled :
            (lower : ℚ) * (2 : ℚ) ^ destinationExponent ≤
              (upper : ℚ) * (2 : ℚ) ^ destinationExponent :=
          lowerBound.trans upperBound
        have ordered : (lower : ℚ) ≤ upper := by
          nlinarith [scaled, positive]
        exact_mod_cast ordered)).denote destinationExponent :=
  ⟨lowerBound, upperBound⟩

end Pixels
