import Mathlib

namespace Pixels

/--
The compiler contract for the folded, degree-11 source-f32 trigonometric
polynomial. The factors are the same versioned constants used by Rust.
-/
structure DerivedDeformContract where
  amplitude : ℝ
  gradient : ℝ
  hessian : ℝ
  thirdDerivative : ℝ

def deriveSinusoidalContract (amplitude frequency : ℝ) : DerivedDeformContract where
  amplitude := 4 * |amplitude|
  gradient := 8 * |amplitude * frequency|
  hessian := 32 * |amplitude * frequency * frequency|
  thirdDerivative := 128 * |amplitude * frequency * frequency * frequency|

/-- The exact algebraic shape evaluated after source range reduction/folding. -/
def sourceFoldedPolynomial (x : ℝ) : ℝ :=
  x + (-0.16666667 : ℝ) * x ^ 3 +
    0.008333333 * x ^ 5 +
    (-0.0001984127 : ℝ) * x ^ 7 +
    0.000002755732 * x ^ 9 +
    (-0.000000025052108 : ℝ) * x ^ 11

def sourceFoldedPolynomialD1 (x : ℝ) : ℝ :=
  1 + 3 * (-0.16666667 : ℝ) * x ^ 2 +
    5 * 0.008333333 * x ^ 4 +
    7 * (-0.0001984127 : ℝ) * x ^ 6 +
    9 * 0.000002755732 * x ^ 8 +
    11 * (-0.000000025052108 : ℝ) * x ^ 10

def sourceFoldedPolynomialD2 (x : ℝ) : ℝ :=
  6 * (-0.16666667 : ℝ) * x +
    20 * 0.008333333 * x ^ 3 +
    42 * (-0.0001984127 : ℝ) * x ^ 5 +
    72 * 0.000002755732 * x ^ 7 +
    110 * (-0.000000025052108 : ℝ) * x ^ 9

def sourceFoldedPolynomialD3 (x : ℝ) : ℝ :=
  6 * (-0.16666667 : ℝ) +
    60 * 0.008333333 * x ^ 2 +
    210 * (-0.0001984127 : ℝ) * x ^ 4 +
    504 * 0.000002755732 * x ^ 6 +
    990 * (-0.000000025052108 : ℝ) * x ^ 8

private theorem abs_sum6
    (a b c d e f : ℝ) :
    |a + b + c + d + e + f| ≤
      |a| + |b| + |c| + |d| + |e| + |f| := by
  calc
    |a + b + c + d + e + f| ≤ |a + b + c + d + e| + |f| := abs_add_le _ _
    _ ≤ (|a + b + c + d| + |e|) + |f| := by
      gcongr
      exact abs_add_le _ _
    _ ≤ ((|a + b + c| + |d|) + |e|) + |f| := by
      gcongr
      exact abs_add_le _ _
    _ ≤ (((|a + b| + |c|) + |d|) + |e|) + |f| := by
      gcongr
      exact abs_add_le _ _
    _ ≤ ((((|a| + |b|) + |c|) + |d|) + |e|) + |f| := by
      gcongr
      exact abs_add_le _ _
    _ = |a| + |b| + |c| + |d| + |e| + |f| := by ring

private theorem abs_sum5
    (a b c d e : ℝ) :
    |a + b + c + d + e| ≤ |a| + |b| + |c| + |d| + |e| := by
  calc
    |a + b + c + d + e| ≤ |a + b + c + d| + |e| := abs_add_le _ _
    _ ≤ (|a + b + c| + |d|) + |e| := by
      gcongr
      exact abs_add_le _ _
    _ ≤ ((|a + b| + |c|) + |d|) + |e| := by
      gcongr
      exact abs_add_le _ _
    _ ≤ (((|a| + |b|) + |c|) + |d|) + |e| := by
      gcongr
      exact abs_add_le _ _
    _ = |a| + |b| + |c| + |d| + |e| := by ring

private theorem abs_pow_le_two_pow (x : ℝ) (hx : |x| ≤ 2) (n : Nat) :
    |x ^ n| ≤ (2 : ℝ) ^ n := by
  rw [abs_pow]
  exact pow_le_pow_left₀ (abs_nonneg x) hx n

/--
The folded core is in `[-π/2,π/2] ⊂ [-2,2]`. These algebraic bounds are
deliberately wider than the real derivative extrema so the corresponding Rust
constants can also include source-f32 operation rounding.
-/
theorem sourceFoldedPolynomial_value_bound (x : ℝ) (hx : |x| ≤ 2) :
    |sourceFoldedPolynomial x| ≤ 4 := by
  have h3 := abs_pow_le_two_pow x hx 3
  have h5 := abs_pow_le_two_pow x hx 5
  have h7 := abs_pow_le_two_pow x hx 7
  have h9 := abs_pow_le_two_pow x hx 9
  have h11 := abs_pow_le_two_pow x hx 11
  unfold sourceFoldedPolynomial
  calc
    |x + -0.16666667 * x ^ 3 + 0.008333333 * x ^ 5 +
        -0.0001984127 * x ^ 7 + 0.000002755732 * x ^ 9 +
        -0.000000025052108 * x ^ 11| ≤
      |x| + |-0.16666667 * x ^ 3| + |0.008333333 * x ^ 5| +
        |-0.0001984127 * x ^ 7| + |0.000002755732 * x ^ 9| +
        |-0.000000025052108 * x ^ 11| := abs_sum6 _ _ _ _ _ _
    _ = |x| + 0.16666667 * |x ^ 3| + 0.008333333 * |x ^ 5| +
        0.0001984127 * |x ^ 7| + 0.000002755732 * |x ^ 9| +
        0.000000025052108 * |x ^ 11| := by
      repeat' rw [abs_mul]
      norm_num
    _ ≤ 4 := by nlinarith

theorem sourceFoldedPolynomial_d1_bound (x : ℝ) (hx : |x| ≤ 2) :
    |sourceFoldedPolynomialD1 x| ≤ 8 := by
  have h2 := abs_pow_le_two_pow x hx 2
  have h4 := abs_pow_le_two_pow x hx 4
  have h6 := abs_pow_le_two_pow x hx 6
  have h8 := abs_pow_le_two_pow x hx 8
  have h10 := abs_pow_le_two_pow x hx 10
  unfold sourceFoldedPolynomialD1
  calc
    |1 + 3 * -0.16666667 * x ^ 2 + 5 * 0.008333333 * x ^ 4 +
        7 * -0.0001984127 * x ^ 6 + 9 * 0.000002755732 * x ^ 8 +
        11 * -0.000000025052108 * x ^ 10| ≤
      |1| + |(3 * -0.16666667) * x ^ 2| +
        |(5 * 0.008333333) * x ^ 4| +
        |(7 * -0.0001984127) * x ^ 6| +
        |(9 * 0.000002755732) * x ^ 8| +
        |(11 * -0.000000025052108) * x ^ 10| := by
          convert abs_sum6 _ _ _ _ _ _ using 1
    _ = 1 + 0.50000001 * |x ^ 2| + 0.041666665 * |x ^ 4| +
        0.0013888889 * |x ^ 6| + 0.000024801588 * |x ^ 8| +
        0.000000275573188 * |x ^ 10| := by
      repeat' rw [abs_mul]
      norm_num
    _ ≤ 8 := by nlinarith

theorem sourceFoldedPolynomial_d2_bound (x : ℝ) (hx : |x| ≤ 2) :
    |sourceFoldedPolynomialD2 x| ≤ 32 := by
  have h3 := abs_pow_le_two_pow x hx 3
  have h5 := abs_pow_le_two_pow x hx 5
  have h7 := abs_pow_le_two_pow x hx 7
  have h9 := abs_pow_le_two_pow x hx 9
  unfold sourceFoldedPolynomialD2
  calc
    |6 * -0.16666667 * x + 20 * 0.008333333 * x ^ 3 +
        42 * -0.0001984127 * x ^ 5 + 72 * 0.000002755732 * x ^ 7 +
        110 * -0.000000025052108 * x ^ 9| ≤
      |(6 * -0.16666667) * x| + |(20 * 0.008333333) * x ^ 3| +
        |(42 * -0.0001984127) * x ^ 5| +
        |(72 * 0.000002755732) * x ^ 7| +
        |(110 * -0.000000025052108) * x ^ 9| := by
          convert abs_sum5 _ _ _ _ _ using 1
    _ = 1.00000002 * |x| + 0.16666666 * |x ^ 3| +
        0.0083333334 * |x ^ 5| + 0.000198412704 * |x ^ 7| +
        0.00000275573188 * |x ^ 9| := by
      repeat' rw [abs_mul]
      norm_num
    _ ≤ 32 := by nlinarith

theorem sourceFoldedPolynomial_d3_bound (x : ℝ) (hx : |x| ≤ 2) :
    |sourceFoldedPolynomialD3 x| ≤ 128 := by
  have h2 := abs_pow_le_two_pow x hx 2
  have h4 := abs_pow_le_two_pow x hx 4
  have h6 := abs_pow_le_two_pow x hx 6
  have h8 := abs_pow_le_two_pow x hx 8
  unfold sourceFoldedPolynomialD3
  calc
    |6 * -0.16666667 + 60 * 0.008333333 * x ^ 2 +
        210 * -0.0001984127 * x ^ 4 + 504 * 0.000002755732 * x ^ 6 +
        990 * -0.000000025052108 * x ^ 8| ≤
      |6 * -0.16666667| + |(60 * 0.008333333) * x ^ 2| +
        |(210 * -0.0001984127) * x ^ 4| +
        |(504 * 0.000002755732) * x ^ 6| +
        |(990 * -0.000000025052108) * x ^ 8| := by
          convert abs_sum5 _ _ _ _ _ using 1
    _ = 1.00000002 + 0.49999998 * |x ^ 2| +
        0.041666667 * |x ^ 4| + 0.001388888928 * |x ^ 6| +
        0.00002480158692 * |x ^ 8| := by
      repeat' rw [abs_mul]
      norm_num
    _ ≤ 128 := by nlinarith

private theorem mulBound
    (x y factor : ℝ) (hy : |y| ≤ factor) :
    |x * y| ≤ factor * |x| := by
  rw [abs_mul]
  simpa [mul_comm] using
    (mul_le_mul_of_nonneg_left hy (abs_nonneg x))

def sinusoidalDisplacement (amplitude core : ℝ) : ℝ :=
  amplitude * sourceFoldedPolynomial core

def sinusoidalGradient (amplitude frequency core : ℝ) : ℝ :=
  (amplitude * frequency) * sourceFoldedPolynomialD1 core

def sinusoidalHessian (amplitude frequency core : ℝ) : ℝ :=
  (amplitude * frequency * frequency) * sourceFoldedPolynomialD2 core

def sinusoidalThirdDerivative (amplitude frequency core : ℝ) : ℝ :=
  (amplitude * frequency * frequency * frequency) * sourceFoldedPolynomialD3 core

theorem sinusoidalAmplitudeBound
    (amplitude frequency core : ℝ) (hcore : |core| ≤ 2) :
    |sinusoidalDisplacement amplitude core| ≤
      (deriveSinusoidalContract amplitude frequency).amplitude := by
  simpa [sinusoidalDisplacement, deriveSinusoidalContract, mul_comm] using
    mulBound amplitude (sourceFoldedPolynomial core) 4
      (sourceFoldedPolynomial_value_bound core hcore)

theorem sinusoidalGradientBound
    (amplitude frequency core : ℝ) (hcore : |core| ≤ 2) :
    |sinusoidalGradient amplitude frequency core| ≤
      (deriveSinusoidalContract amplitude frequency).gradient := by
  simpa [sinusoidalGradient, deriveSinusoidalContract, mul_comm] using
    mulBound (amplitude * frequency) (sourceFoldedPolynomialD1 core) 8
      (sourceFoldedPolynomial_d1_bound core hcore)

theorem sinusoidalHessianBound
    (amplitude frequency core : ℝ) (hcore : |core| ≤ 2) :
    |sinusoidalHessian amplitude frequency core| ≤
      (deriveSinusoidalContract amplitude frequency).hessian := by
  simpa [sinusoidalHessian, deriveSinusoidalContract, mul_comm] using
    mulBound (amplitude * frequency * frequency)
      (sourceFoldedPolynomialD2 core) 32
      (sourceFoldedPolynomial_d2_bound core hcore)

theorem sinusoidalThirdDerivativeBound
    (amplitude frequency core : ℝ) (hcore : |core| ≤ 2) :
    |sinusoidalThirdDerivative amplitude frequency core| ≤
      (deriveSinusoidalContract amplitude frequency).thirdDerivative := by
  simpa [sinusoidalThirdDerivative, deriveSinusoidalContract, mul_comm] using
    mulBound (amplitude * frequency * frequency * frequency)
      (sourceFoldedPolynomialD3 core) 128
      (sourceFoldedPolynomial_d3_bound core hcore)

end Pixels
