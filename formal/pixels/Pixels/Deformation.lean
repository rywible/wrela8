import Mathlib

namespace Pixels

/--
The four compiler-derived absolute derivative bounds for the closed
sinusoidal deformation form. Source authors do not provide these values.
-/
structure DerivedDeformContract where
  amplitude : ℝ
  gradient : ℝ
  hessian : ℝ
  thirdDerivative : ℝ

def deriveSinusoidalContract (amplitude frequency : ℝ) : DerivedDeformContract where
  amplitude := |amplitude|
  gradient := |amplitude * frequency|
  hessian := |amplitude * frequency * frequency|
  thirdDerivative := |amplitude * frequency * frequency * frequency|

private theorem mulUnitBound (x y : ℝ) (hy : |y| ≤ 1) :
    |x * y| ≤ |x| := by
  rw [abs_mul]
  nlinarith [abs_nonneg x, abs_nonneg y]

def sinusoidalPhase (frequency phase x : ℝ) : ℝ :=
  frequency * x + phase

noncomputable def sinusoidalDisplacement
    (amplitude frequency phase x : ℝ) : ℝ :=
  amplitude * Real.sin (sinusoidalPhase frequency phase x)

noncomputable def sinusoidalGradient
    (amplitude frequency phase x : ℝ) : ℝ :=
  (amplitude * frequency) * Real.cos (sinusoidalPhase frequency phase x)

noncomputable def sinusoidalHessian
    (amplitude frequency phase x : ℝ) : ℝ :=
  -(amplitude * frequency * frequency) *
    Real.sin (sinusoidalPhase frequency phase x)

noncomputable def sinusoidalThirdDerivative
    (amplitude frequency phase x : ℝ) : ℝ :=
  -(amplitude * frequency * frequency * frequency) *
    Real.cos (sinusoidalPhase frequency phase x)

theorem sinusoidalAmplitudeBound (amplitude frequency phase x : ℝ) :
    |sinusoidalDisplacement amplitude frequency phase x| ≤
      (deriveSinusoidalContract amplitude frequency).amplitude := by
  simpa [sinusoidalDisplacement, deriveSinusoidalContract] using
    mulUnitBound amplitude (Real.sin (sinusoidalPhase frequency phase x))
      (Real.abs_sin_le_one _)

theorem sinusoidalGradientBound (amplitude frequency phase x : ℝ) :
    |sinusoidalGradient amplitude frequency phase x| ≤
      (deriveSinusoidalContract amplitude frequency).gradient := by
  simpa [sinusoidalGradient, deriveSinusoidalContract] using
    mulUnitBound (amplitude * frequency)
      (Real.cos (sinusoidalPhase frequency phase x)) (Real.abs_cos_le_one _)

theorem sinusoidalHessianBound (amplitude frequency phase x : ℝ) :
    |sinusoidalHessian amplitude frequency phase x| ≤
      (deriveSinusoidalContract amplitude frequency).hessian := by
  simpa [sinusoidalHessian, deriveSinusoidalContract, abs_neg] using
    mulUnitBound (amplitude * frequency * frequency)
      (Real.sin (sinusoidalPhase frequency phase x)) (Real.abs_sin_le_one _)

theorem sinusoidalThirdDerivativeBound (amplitude frequency phase x : ℝ) :
    |sinusoidalThirdDerivative amplitude frequency phase x| ≤
      (deriveSinusoidalContract amplitude frequency).thirdDerivative := by
  simpa [sinusoidalThirdDerivative, deriveSinusoidalContract, abs_neg] using
    mulUnitBound (amplitude * frequency * frequency * frequency)
      (Real.cos (sinusoidalPhase frequency phase x)) (Real.abs_cos_le_one _)

end Pixels
