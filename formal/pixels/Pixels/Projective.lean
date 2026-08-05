import Mathlib

namespace Pixels

structure Vec3 where
  x : ℝ
  y : ℝ
  z : ℝ

@[ext]
theorem Vec3.ext {a b : Vec3}
    (hx : a.x = b.x) (hy : a.y = b.y) (hz : a.z = b.z) : a = b := by
  cases a
  cases b
  simp_all

def Vec3.add (a b : Vec3) : Vec3 :=
  ⟨a.x + b.x, a.y + b.y, a.z + b.z⟩

def Vec3.scale (s : ℝ) (a : Vec3) : Vec3 :=
  ⟨s * a.x, s * a.y, s * a.z⟩

def Vec3.dot (a b : Vec3) : ℝ :=
  a.x * b.x + a.y * b.y + a.z * b.z

noncomputable def Vec3.normalizeBy (a : Vec3) (length : ℝ) : Vec3 :=
  a.scale length⁻¹

def rawRay (forward right up : Vec3) (u v : ℝ) : Vec3 :=
  forward.add ((right.scale u).add (up.scale v))

noncomputable def projectivePoint (eye ray : Vec3) (q : ℝ) : Vec3 :=
  eye.add (ray.scale q⁻¹)

def planeValue (normal point : Vec3) (offset : ℝ) : ℝ :=
  normal.dot point + offset

def homogeneousPlane
    (normal eye ray : Vec3) (offset q : ℝ) : ℝ :=
  normal.dot ray + q * (normal.dot eye + offset)

/-- Multiplying the source plane equation by positive inverse depth yields the
exact affine homogeneous projective equation emitted by the compiler. -/
theorem projectivePlaneCancellation
    (normal eye ray : Vec3) (offset q : ℝ) (hq : q ≠ 0) :
    q * planeValue normal (projectivePoint eye ray q) offset =
      homogeneousPlane normal eye ray offset q := by
  simp only [planeValue, projectivePoint, Vec3.dot, Vec3.add, Vec3.scale]
  field_simp
  simp only [homogeneousPlane, Vec3.dot]
  ring

/-- The canonical orthonormal camera equations make inverse depth equal to
view-forward depth without normalizing the per-pixel ray. -/
theorem canonicalRayForwardComponent
    (forward right up : Vec3) (u v : ℝ)
    (hforward : forward.dot forward = 1)
    (hright : right.dot forward = 0)
    (hup : up.dot forward = 0) :
    (rawRay forward right up u v).dot forward = 1 := by
  rw [show (rawRay forward right up u v).dot forward =
      forward.dot forward + u * right.dot forward + v * up.dot forward by
    simp only [rawRay, Vec3.dot, Vec3.add, Vec3.scale]
    ring]
  rw [hforward, hright, hup]
  ring

/-- Normalizing a camera ray and then converting view-forward depth back to
distance along that normalized ray cancels the normalization exactly. This is
why the compiler can use the cheaper unnormalized ray with inverse view depth:
`dot(rawRay, forward) = 1` makes its projective parameter exactly `q`. -/
theorem normalizedRayCameraCancellation
    (ray forward : Vec3) (z length : ℝ)
    (hlength : length ≠ 0)
    (hforward : ray.dot forward = 1) :
    (ray.normalizeBy length).scale
        (z / ((ray.normalizeBy length).dot forward)) =
      ray.scale z := by
  cases ray with
  | mk rayX rayY rayZ =>
    cases forward with
    | mk forwardX forwardY forwardZ =>
      simp only [Vec3.normalizeBy, Vec3.scale, Vec3.dot] at hforward ⊢
      ext <;> dsimp
      all_goals field_simp [hlength]
      all_goals rw [hforward]
      all_goals ring

end Pixels
