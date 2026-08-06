import Mathlib

namespace Pixels

def outwardArea (exactArea scale : ℝ) : Set ℝ :=
  Set.Icc ((Int.floor (exactArea * scale) : ℤ) : ℝ)
    ((Int.ceil (exactArea * scale) : ℤ) : ℝ)

theorem line_coverage_interval
    (exactArea scale : ℝ) :
    exactArea * scale ∈
      Set.Icc ((Int.floor (exactArea * scale) : ℤ) : ℝ)
        ((Int.ceil (exactArea * scale) : ℤ) : ℝ) := by
  exact ⟨Int.floor_le _, Int.le_ceil _⟩

theorem half_plane_area_enclosure
    (clippedPolygonArea scale : ℝ) :
    clippedPolygonArea * scale ∈ outwardArea clippedPolygonArea scale :=
  line_coverage_interval clippedPolygonArea scale

theorem monotone_curve_strip_enclosure
    (curveArea lineArea stripRadius : ℝ)
    (stripBound : |curveArea - lineArea| ≤ stripRadius) :
    curveArea ∈ Set.Icc (lineArea - stripRadius) (lineArea + stripRadius) := by
  rw [abs_le] at stripBound
  constructor <;> linarith

inductive BoundaryOwner where
  | lowerOrLeft
  | upperOrRight

def ownsBoundary (owner : BoundaryOwner) (signedDistance : ℝ) : Prop :=
  signedDistance < 0 ∨
    (signedDistance = 0 ∧ owner = .lowerOrLeft)

theorem half_open_boundary_exactly_one_owner :
    ownsBoundary .lowerOrLeft 0 ∧
      ¬ ownsBoundary .upperOrRight 0 := by
  simp [ownsBoundary]

theorem monotone_piece_union_preserves_area_bounds
    (first second exactFirst exactSecond firstRadius secondRadius : ℝ)
    (firstBound : |exactFirst - first| ≤ firstRadius)
    (secondBound : |exactSecond - second| ≤ secondRadius) :
    |(exactFirst + exactSecond) - (first + second)| ≤
      firstRadius + secondRadius := by
  calc
    _ = |(exactFirst - first) + (exactSecond - second)| := by ring_nf
    _ ≤ |exactFirst - first| + |exactSecond - second| := abs_add_le _ _
    _ ≤ firstRadius + secondRadius := add_le_add firstBound secondBound

theorem quadratic_stationary_split_exact
    (a b : ℝ) (nondegenerate : a ≠ 0) :
    b + 2 * a * (-b / (2 * a)) = 0 := by
  field_simp [nondegenerate]
  ring

theorem coverage_color_error
    (alpha exactAlpha front back : ℝ)
    (coverageError : |alpha - exactAlpha| ≤ ε) :
    |(alpha * front + (1 - alpha) * back) -
      (exactAlpha * front + (1 - exactAlpha) * back)| ≤
      ε * |front - back| := by
  have factor :
      (alpha * front + (1 - alpha) * back) -
        (exactAlpha * front + (1 - exactAlpha) * back) =
        (alpha - exactAlpha) * (front - back) := by
    ring
  calc
    _ = |alpha - exactAlpha| * |front - back| := by rw [factor, abs_mul]
    _ ≤ ε * |front - back| :=
      mul_le_mul_of_nonneg_right coverageError (abs_nonneg _)

end Pixels
