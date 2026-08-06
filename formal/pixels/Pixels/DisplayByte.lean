import Pixels.Interval

namespace Pixels

def colorMatrixChannel
    (c0 c1 c2 m0 m1 m2 : ℝ) : ℝ :=
  c0 * m0 + c1 * m1 + c2 * m2

theorem color_matrix_channel_expansion
    (c0 c1 c2 m0 m1 m2 : ℝ) :
    colorMatrixChannel c0 c1 c2 m0 m1 m2 =
      c0 * m0 + c1 * m1 + c2 * m2 := by
  rfl

theorem monotone_lut_endpoint_enclosure
    (interpolate : ℝ → ℝ) (lo x hi : ℝ)
    (monotone : Monotone interpolate)
    (contained : lo ≤ x ∧ x ≤ hi) :
    interpolate lo ≤ interpolate x ∧
      interpolate x ≤ interpolate hi :=
  ⟨monotone contained.1, monotone contained.2⟩

def tiesEvenIncrement (quotient remainder half : Nat) : Nat :=
  if remainder > half ∨
      (remainder = half ∧ quotient % 2 = 1) then 1 else 0

theorem quantize_ties_even_model
    (quotient remainder half : Nat) :
    quotient + tiesEvenIncrement quotient remainder half =
      if remainder > half ∨
          (remainder = half ∧ quotient % 2 = 1)
        then quotient + 1 else quotient := by
  by_cases increment :
      remainder > half ∨ (remainder = half ∧ quotient % 2 = 1)
  · simp [tiesEvenIncrement, increment]
  · simp [tiesEvenIncrement, increment]

theorem display_singleton
    (encode : ℝ → Nat) (lo x hi : ℝ)
    (monotone : Monotone encode)
    (contained : lo ≤ x ∧ x ≤ hi)
    (same : encode lo = encode hi) :
    encode x = encode lo := by
  apply Nat.le_antisymm
  · rw [same]
    exact monotone contained.2
  · exact monotone contained.1

def RgbSingleton
    (encode : ℝ → Nat) (lower upper : Fin 3 → ℝ)
    (codes : Fin 3 → Nat) : Prop :=
  ∀ channel, encode (lower channel) = codes channel ∧
    encode (upper channel) = codes channel

theorem rgb_singleton_is_exact
    (encode : ℝ → Nat) (lower sample upper : Fin 3 → ℝ)
    (codes : Fin 3 → Nat)
    (monotone : Monotone encode)
    (contained :
      ∀ channel, lower channel ≤ sample channel ∧
        sample channel ≤ upper channel)
    (singleton : RgbSingleton encode lower upper codes) :
    ∀ channel, encode (sample channel) = codes channel := by
  intro channel
  have exactChannel := display_singleton encode
    (lower channel) (sample channel) (upper channel)
    monotone (contained channel)
    ((singleton channel).1.trans (singleton channel).2.symm)
  exact exactChannel.trans (singleton channel).1

theorem exposure_channel_contains
    (color multiplier : Interval) (value exposure : ℝ)
    (colorContains : color.Contains value)
    (multiplierContains : multiplier.Contains exposure) :
    (color.mulHull multiplier).Contains (value * exposure) :=
  Interval.mulHull_contains color multiplier value exposure
    colorContains multiplierContains

end Pixels
