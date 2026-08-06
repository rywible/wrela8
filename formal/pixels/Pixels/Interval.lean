import Mathlib

namespace Pixels

structure Interval where
  lo : ℝ
  hi : ℝ
  ordered : lo ≤ hi

def Interval.Contains (interval : Interval) (value : ℝ) : Prop :=
  interval.lo ≤ value ∧ value ≤ interval.hi

theorem Interval.contains_zero_model (interval : Interval) :
    interval.Contains 0 ↔ interval.lo ≤ 0 ∧ 0 ≤ interval.hi := by
  rfl

theorem Interval.strict_positive_model (interval : Interval) :
    0 < interval.lo → ∀ value, interval.Contains value → 0 < value := by
  intro positive value contained
  exact lt_of_lt_of_le positive contained.1

theorem Interval.strict_negative_model (interval : Interval) :
    interval.hi < 0 → ∀ value, interval.Contains value → value < 0 := by
  intro negative value contained
  exact lt_of_le_of_lt contained.2 negative

structure RawDomain where
  min : Int
  max : Int

def RawDomain.Contains (domain : RawDomain) (value : Int) : Prop :=
  domain.min ≤ value ∧ value ≤ domain.max

noncomputable def RawDomain.checkedAdd
    (domain : RawDomain) (left right : Int) : Option Int :=
  by
    classical
    exact if domain.Contains left ∧ domain.Contains right ∧
        domain.Contains (left + right) then
      some (left + right)
    else
      none

theorem Interval.restricted_add_contract
    (domain : RawDomain) (left right : Int) :
    domain.checkedAdd left right = some (left + right) ↔
      domain.Contains left ∧ domain.Contains right ∧
        domain.Contains (left + right) := by
  classical
  simp [RawDomain.checkedAdd]

structure FixedDomainModel where
  exponent : Int
  min : Int
  max : Int

def FixedDomainModel.Valid (domain : FixedDomainModel) : Prop :=
  -96 ≤ domain.exponent ∧ domain.exponent ≤ 63 ∧ domain.min ≤ domain.max

noncomputable def checkedFixedDomain
    (exponent min max : Int) : Option FixedDomainModel :=
  by
    classical
    let domain := FixedDomainModel.mk exponent min max
    exact if domain.Valid then some domain else none

theorem fixed_domain_contract (exponent min max : Int) :
    checkedFixedDomain exponent min max =
        some (FixedDomainModel.mk exponent min max) ↔
      -96 ≤ exponent ∧ exponent ≤ 63 ∧ min ≤ max := by
  classical
  simp [checkedFixedDomain, FixedDomainModel.Valid]

def Interval.add (a b : Interval) : Interval where
  lo := a.lo + b.lo
  hi := a.hi + b.hi
  ordered := by linarith [a.ordered, b.ordered]

theorem Interval.add_contains
    (a b : Interval) (x y : ℝ)
    (hx : a.Contains x) (hy : b.Contains y) :
    (a.add b).Contains (x + y) := by
  constructor <;> simp only [add, Contains] at * <;> linarith

noncomputable def Interval.scalePow2
    (interval : Interval) (shift : Int) : Interval where
  lo := interval.lo * (2 : ℝ) ^ shift
  hi := interval.hi * (2 : ℝ) ^ shift
  ordered := mul_le_mul_of_nonneg_right interval.ordered
    (le_of_lt (zpow_pos (by norm_num) shift))

theorem Interval.scale_pow2_contains
    (interval : Interval) (value : ℝ) (shift : Int)
    (contained : interval.Contains value) :
    (interval.scalePow2 shift).Contains (value * (2 : ℝ) ^ shift) := by
  constructor
  · exact mul_le_mul_of_nonneg_right contained.1
      (le_of_lt (zpow_pos (by norm_num) shift))
  · exact mul_le_mul_of_nonneg_right contained.2
      (le_of_lt (zpow_pos (by norm_num) shift))

def Interval.neg (a : Interval) : Interval where
  lo := -a.hi
  hi := -a.lo
  ordered := by linarith [a.ordered]

theorem Interval.neg_contains
    (a : Interval) (x : ℝ) (hx : a.Contains x) :
    a.neg.Contains (-x) := by
  constructor <;> simp only [neg, Contains] at * <;> linarith

def Interval.sub (a b : Interval) : Interval :=
  a.add b.neg

theorem Interval.sub_contains
    (a b : Interval) (x y : ℝ)
    (hx : a.Contains x) (hy : b.Contains y) :
    (a.sub b).Contains (x - y) := by
  simpa [sub, sub_eq_add_neg] using
    Interval.add_contains a b.neg x (-y) hx (Interval.neg_contains b y hy)

def Interval.minInterval (a b : Interval) : Interval where
  lo := min a.lo b.lo
  hi := min a.hi b.hi
  ordered := min_le_min a.ordered b.ordered

theorem Interval.min_contains
    (a b : Interval) (x y : ℝ)
    (hx : a.Contains x) (hy : b.Contains y) :
    (a.minInterval b).Contains (min x y) := by
  exact ⟨min_le_min hx.1 hy.1, min_le_min hx.2 hy.2⟩

def Interval.maxInterval (a b : Interval) : Interval where
  lo := max a.lo b.lo
  hi := max a.hi b.hi
  ordered := max_le_max a.ordered b.ordered

theorem Interval.max_contains
    (a b : Interval) (x y : ℝ)
    (hx : a.Contains x) (hy : b.Contains y) :
    (a.maxInterval b).Contains (max x y) := by
  exact ⟨max_le_max hx.1 hy.1, max_le_max hx.2 hy.2⟩

theorem square_nonnegative (x : ℝ) : 0 ≤ x * x := by
  nlinarith [sq_nonneg x]

def Interval.absRadius (a : Interval) : ℝ :=
  max |a.lo| |a.hi|

theorem Interval.abs_le_absRadius
    (a : Interval) (x : ℝ) (hx : a.Contains x) :
    |x| ≤ a.absRadius := by
  apply abs_le.mpr
  constructor
  · have hlo : -a.absRadius ≤ a.lo := by
      simp only [absRadius]
      nlinarith [neg_le_abs a.lo, le_max_left |a.lo| |a.hi|]
    linarith [hx.1]
  · have hhi : a.hi ≤ a.absRadius := by
      simp only [absRadius]
      nlinarith [le_abs_self a.hi, le_max_right |a.lo| |a.hi|]
    linarith [hx.2]

def Interval.absInterval (a : Interval) : Interval where
  lo := 0
  hi := a.absRadius
  ordered := by
    simp only [absRadius]
    exact le_trans (abs_nonneg a.lo) (le_max_left _ _)

theorem Interval.abs_contains
    (a : Interval) (x : ℝ) (hx : a.Contains x) :
    a.absInterval.Contains |x| :=
  ⟨abs_nonneg x, a.abs_le_absRadius x hx⟩

def Interval.mulHull (a b : Interval) : Interval where
  lo := -(a.absRadius * b.absRadius)
  hi := a.absRadius * b.absRadius
  ordered := by
    have ha : 0 ≤ a.absRadius :=
      le_trans (abs_nonneg a.lo) (le_max_left _ _)
    have hb : 0 ≤ b.absRadius :=
      le_trans (abs_nonneg b.lo) (le_max_left _ _)
    nlinarith [mul_nonneg ha hb]

theorem Interval.mulHull_contains
    (a b : Interval) (x y : ℝ)
    (hx : a.Contains x) (hy : b.Contains y) :
    (a.mulHull b).Contains (x * y) := by
  have hxa := a.abs_le_absRadius x hx
  have hya := b.abs_le_absRadius y hy
  have ha : 0 ≤ a.absRadius :=
    le_trans (abs_nonneg a.lo) (le_max_left _ _)
  have hxy : |x * y| ≤ a.absRadius * b.absRadius := by
    rw [abs_mul]
    exact mul_le_mul hxa hya (abs_nonneg y) ha
  exact (abs_le.mp hxy)

theorem Interval.affine_contains
    (input scale bias : Interval) (x multiplier offset : ℝ)
    (hx : input.Contains x)
    (hm : scale.Contains multiplier)
    (ho : bias.Contains offset) :
    ((scale.mulHull input).add bias).Contains
      (multiplier * x + offset) :=
  Interval.add_contains _ _ _ _
    (Interval.mulHull_contains scale input _ _ hm hx) ho

def Interval.squareInterval (a : Interval) : Interval where
  lo := 0
  hi := a.absRadius * a.absRadius
  ordered := mul_self_nonneg _

theorem Interval.square_contains
    (a : Interval) (x : ℝ) (hx : a.Contains x) :
    a.squareInterval.Contains (x * x) := by
  constructor
  · exact square_nonnegative x
  · have h := a.abs_le_absRadius x hx
    have hmul := mul_self_le_mul_self (abs_nonneg x) h
    have heq : |x| * |x| = x * x := by
      rw [← abs_mul, abs_of_nonneg (square_nonnegative x)]
    rw [← heq]
    exact hmul

def Interval.clampInterval (value lo hi : Interval) : Interval :=
  (value.maxInterval lo).minInterval hi

theorem Interval.clamp_contains
    (value lo hi : Interval) (x lower upper : ℝ)
    (hx : value.Contains x) (hl : lo.Contains lower) (hu : hi.Contains upper) :
    (value.clampInterval lo hi).Contains (min (max x lower) upper) := by
  exact Interval.min_contains _ _ _ _ (Interval.max_contains _ _ _ _ hx hl) hu

noncomputable def Interval.sqrtInterval (a : Interval) (_h : 0 ≤ a.lo) : Interval where
  lo := Real.sqrt a.lo
  hi := Real.sqrt a.hi
  ordered := Real.sqrt_le_sqrt a.ordered

theorem Interval.sqrt_contains
    (a : Interval) (h : 0 ≤ a.lo) (x : ℝ) (hx : a.Contains x) :
    (a.sqrtInterval h).Contains (Real.sqrt x) :=
  ⟨Real.sqrt_le_sqrt hx.1, Real.sqrt_le_sqrt hx.2⟩

def Interval.hull (a b : Interval) : Interval where
  lo := min a.lo b.lo
  hi := max a.hi b.hi
  ordered := le_trans (min_le_left _ _) (le_trans a.ordered (le_max_left _ _))

theorem Interval.hull_contains_left
    (a b : Interval) (x : ℝ) (hx : a.Contains x) :
    (a.hull b).Contains x :=
  ⟨le_trans (min_le_left _ _) hx.1, le_trans hx.2 (le_max_left _ _)⟩

theorem Interval.hull_contains_right
    (a b : Interval) (x : ℝ) (hx : b.Contains x) :
    (a.hull b).Contains x :=
  ⟨le_trans (min_le_right _ _) hx.1, le_trans hx.2 (le_max_right _ _)⟩

def Interval.symmetric (radius : ℝ) (h : 0 ≤ radius) : Interval where
  lo := -radius
  hi := radius
  ordered := by linarith

theorem Interval.reciprocal_contains_of_abs_bound
    (radius x : ℝ) (hr : 0 ≤ radius)
    (hx : |x⁻¹| ≤ radius) :
    (Interval.symmetric radius hr).Contains x⁻¹ := by
  exact abs_le.mp hx

theorem Interval.div_contains_of_abs_bound
    (radius x y : ℝ) (hr : 0 ≤ radius)
    (hxy : |x / y| ≤ radius) :
    (Interval.symmetric radius hr).Contains (x / y) := by
  exact abs_le.mp hxy

noncomputable def Interval.reciprocalPositive
    (a : Interval) (hpositive : 0 < a.lo) : Interval where
  lo := 1 / a.hi
  hi := 1 / a.lo
  ordered := one_div_le_one_div_of_le hpositive a.ordered

theorem Interval.reciprocalPositive_contains
    (a : Interval) (hpositive : 0 < a.lo)
    (x : ℝ) (hx : a.Contains x) :
    (a.reciprocalPositive hpositive).Contains x⁻¹ := by
  have hxpositive : 0 < x := lt_of_lt_of_le hpositive hx.1
  constructor
  · change 1 / a.hi ≤ x⁻¹
    rw [inv_eq_one_div]
    exact one_div_le_one_div_of_le hxpositive hx.2
  · change x⁻¹ ≤ 1 / a.lo
    rw [inv_eq_one_div]
    exact one_div_le_one_div_of_le hpositive hx.1

noncomputable def Interval.reciprocalNegative
    (a : Interval) (hnegative : a.hi < 0) : Interval :=
  (a.neg.reciprocalPositive (by
    simp only [neg]
    linarith)).neg

theorem Interval.reciprocalNegative_contains
    (a : Interval) (hnegative : a.hi < 0)
    (x : ℝ) (hx : a.Contains x) :
    (a.reciprocalNegative hnegative).Contains x⁻¹ := by
  have hnegContains : a.neg.Contains (-x) := a.neg_contains x hx
  have hpositive : 0 < a.neg.lo := by
    simp only [neg]
    linarith
  have hrecip := a.neg.reciprocalPositive_contains hpositive (-x) hnegContains
  have houter := (a.neg.reciprocalPositive hpositive).neg_contains ((-x)⁻¹) hrecip
  simpa [reciprocalNegative] using houter

noncomputable def Interval.divPositive
    (a b : Interval) (hpositive : 0 < b.lo) : Interval :=
  a.mulHull (b.reciprocalPositive hpositive)

theorem Interval.divPositive_contains
    (a b : Interval) (hpositive : 0 < b.lo)
    (x y : ℝ) (hx : a.Contains x) (hy : b.Contains y) :
    (a.divPositive b hpositive).Contains (x / y) := by
  have hrecip := b.reciprocalPositive_contains hpositive y hy
  simpa [divPositive, div_eq_mul_inv] using
    a.mulHull_contains (b.reciprocalPositive hpositive) x y⁻¹ hx hrecip

def Interval.unit : Interval where
  lo := -1
  hi := 1
  ordered := by norm_num

theorem Interval.sin_contains (x : ℝ) :
    Interval.unit.Contains (Real.sin x) :=
  ⟨Real.neg_one_le_sin x, Real.sin_le_one x⟩

theorem Interval.cos_contains (x : ℝ) :
    Interval.unit.Contains (Real.cos x) :=
  ⟨Real.neg_one_le_cos x, Real.cos_le_one x⟩

def Interval.roundedImage (exact : Interval) (epsilon : ℝ)
    (h : 0 ≤ epsilon) : Interval where
  lo := exact.lo - epsilon
  hi := exact.hi + epsilon
  ordered := by linarith [exact.ordered]

theorem Interval.roundedImage_contains
    (exact : Interval) (epsilon source rounded : ℝ)
    (heps : 0 ≤ epsilon)
    (hsource : exact.Contains source)
    (hround : |rounded - source| ≤ epsilon) :
    (exact.roundedImage epsilon heps).Contains rounded := by
  have h := abs_le.mp hround
  change exact.lo - epsilon ≤ rounded ∧ rounded ≤ exact.hi + epsilon
  exact ⟨by linarith [hsource.1, h.1], by linarith [hsource.2, h.2]⟩

/-- One source-f32 result is related to its exact real expression by a proved
operation-specific error radius. Rust supplies that finite radius from its
outward endpoint implementation. -/
def SourceF32Result (exact rounded epsilon : ℝ) : Prop :=
  0 ≤ epsilon ∧ |rounded - exact| ≤ epsilon

theorem Interval.dot3_source_f32_contains
    (a₀ a₁ a₂ b₀ b₁ b₂ : Interval)
    (x₀ x₁ x₂ y₀ y₁ y₂ rounded epsilon : ℝ)
    (ha₀ : a₀.Contains x₀) (ha₁ : a₁.Contains x₁)
    (ha₂ : a₂.Contains x₂) (hb₀ : b₀.Contains y₀)
    (hb₁ : b₁.Contains y₁) (hb₂ : b₂.Contains y₂)
    (hsource : SourceF32Result
      (x₀ * y₀ + x₁ * y₁ + x₂ * y₂) rounded epsilon) :
    (((a₀.mulHull b₀).add (a₁.mulHull b₁)).add
      (a₂.mulHull b₂)).roundedImage epsilon hsource.1 |>.Contains rounded := by
  have h₀ := a₀.mulHull_contains b₀ x₀ y₀ ha₀ hb₀
  have h₁ := a₁.mulHull_contains b₁ x₁ y₁ ha₁ hb₁
  have h₂ := a₂.mulHull_contains b₂ x₂ y₂ ha₂ hb₂
  have hexact := Interval.add_contains
    ((a₀.mulHull b₀).add (a₁.mulHull b₁)) (a₂.mulHull b₂)
    (x₀ * y₀ + x₁ * y₁) (x₂ * y₂)
    (Interval.add_contains _ _ _ _ h₀ h₁) h₂
  exact Interval.roundedImage_contains _ _ _ _ hsource.1 hexact hsource.2

theorem Interval.cross_source_f32_contains
    (a b c d : Interval) (x y z w rounded epsilon : ℝ)
    (hx : a.Contains x) (hy : b.Contains y)
    (hz : c.Contains z) (hw : d.Contains w)
    (hsource : SourceF32Result (x * y - z * w) rounded epsilon) :
    ((a.mulHull b).sub (c.mulHull d)).roundedImage epsilon hsource.1
      |>.Contains rounded := by
  have hxy := a.mulHull_contains b x y hx hy
  have hzw := c.mulHull_contains d z w hz hw
  have hexact := Interval.sub_contains _ _ _ _ hxy hzw
  exact Interval.roundedImage_contains _ _ _ _ hsource.1 hexact hsource.2

theorem Interval.length3_source_f32_contains
    (a b c : Interval) (x y z rounded epsilon : ℝ)
    (hx : a.Contains x) (hy : b.Contains y) (hz : c.Contains z)
    (hsource : SourceF32Result
      (Real.sqrt (x * x + y * y + z * z)) rounded epsilon) :
    let squared := ((a.squareInterval.add b.squareInterval).add c.squareInterval)
    (squared.sqrtInterval (by
      norm_num [squared, Interval.squareInterval, Interval.add])).roundedImage
      epsilon hsource.1 |>.Contains rounded := by
  dsimp
  have hx2 := a.square_contains x hx
  have hy2 := b.square_contains y hy
  have hz2 := c.square_contains z hz
  have hsum := Interval.add_contains
    (a.squareInterval.add b.squareInterval) c.squareInterval
    (x * x + y * y) (z * z)
    (Interval.add_contains _ _ _ _ hx2 hy2) hz2
  have hsqrt := Interval.sqrt_contains
    ((a.squareInterval.add b.squareInterval).add c.squareInterval)
    (by simp [squareInterval, add]) _ hsum
  exact Interval.roundedImage_contains _ _ _ _ hsource.1 hsqrt hsource.2

theorem Interval.normalize_source_f32_contains_nonzero
    (component length : Interval) (hpositive : 0 < length.lo)
    (x len sourceRounded epsilon : ℝ)
    (hx : component.Contains x) (hlen : length.Contains len)
    (hsource : SourceF32Result (x / len) sourceRounded epsilon) :
    (component.divPositive length hpositive).roundedImage epsilon hsource.1
      |>.Contains sourceRounded := by
  have hexact := component.divPositive_contains length hpositive x len hx hlen
  exact Interval.roundedImage_contains _ _ _ _ hsource.1 hexact hsource.2

theorem Interval.normalize_zero_containing_finite
    (maximum value : ℝ) (hmaximum : 0 ≤ maximum)
    (hfinite : |value| ≤ maximum) :
    (Interval.symmetric maximum hmaximum).Contains value :=
  abs_le.mp hfinite

theorem Interval.select_source_f32_contains
    (a b : Interval) (chooseLeft : Bool) (x y selected : ℝ)
    (hx : a.Contains x) (hy : b.Contains y)
    (hselected : selected = if chooseLeft then x else y) :
    (a.hull b).Contains selected := by
  subst selected
  cases chooseLeft with
  | false => simpa using a.hull_contains_right b y hy
  | true => simpa using a.hull_contains_left b x hx

theorem Interval.smoothMin_source_f32_contains
    (a b exact : Interval) (source rounded epsilon : ℝ)
    (hsource : exact.Contains source)
    (hrounded : SourceF32Result source rounded epsilon) :
    ((exact.hull a).hull b).roundedImage epsilon hrounded.1
      |>.Contains rounded := by
  have hfirst := exact.hull_contains_left a source hsource
  have hhull := (exact.hull a).hull_contains_left b source hfirst
  exact Interval.roundedImage_contains _ _ _ _ hrounded.1 hhull hrounded.2

def Interval.normalizeUnit (epsilon : ℝ) (h : 0 ≤ epsilon) : Interval :=
  Interval.roundedImage Interval.unit epsilon h

theorem Interval.normalize_contains_with_roundoff
    (epsilon value : ℝ) (heps : 0 ≤ epsilon)
    (hvalue : |value| ≤ 1 + epsilon) :
    (Interval.normalizeUnit epsilon heps).Contains value := by
  simp only [normalizeUnit, roundedImage, unit, Contains]
  have h := abs_le.mp hvalue
  constructor <;> linarith

end Pixels
