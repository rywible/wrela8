import Mathlib

namespace Pixels

theorem strict_interval_q_order
    (aLo aHi bLo bHi a b : ℝ)
    (aContains : aLo ≤ a ∧ a ≤ aHi)
    (bContains : bLo ≤ b ∧ b ≤ bHi)
    (separated : bHi < aLo) :
    b < a := by
  linarith

theorem adjacent_q_order_three
    (a b c : ℝ) (ab : b < a) (bc : c < b) :
    c < a := by
  linarith

structure QRootModel where
  lo : ℤ
  hi : ℤ
  feature : Nat
  orientation : Int

def QRootModel.Overlaps (a b : QRootModel) : Prop :=
  a.lo ≤ b.hi ∧ b.lo ≤ a.hi

/-- Reflexive/transitive closure of interval overlap. This is the corridor
component relation used by the executable component sort. -/
inductive SameCorridorComponent : QRootModel → QRootModel → Prop
  | refl (root) : SameCorridorComponent root root
  | overlap {a b} (h : a.Overlaps b) : SameCorridorComponent a b
  | symm {a b} (h : SameCorridorComponent a b) :
      SameCorridorComponent b a
  | trans {a b c} (ab : SameCorridorComponent a b)
      (bc : SameCorridorComponent b c) : SameCorridorComponent a c

def BoundedQRootList (roots : List QRootModel) : Prop :=
  roots.length ≤ 64

/-- A component sort is canonical only when permutations of the same bounded
input produce byte-for-byte identical output. Rust/Wrela implement this by an
endpoint-total first pass, component discovery, then feature/orientation sort
inside each complete component. -/
def CanonicalComponentSort
    (sort : List QRootModel → List QRootModel) : Prop :=
  ∀ left right,
    BoundedQRootList left →
    BoundedQRootList right →
    left.Perm right →
    sort left = sort right

theorem q_corridor_overlap_transitive
    (a b c : QRootModel)
    (ab : a.Overlaps b) (bc : b.Overlaps c) :
    SameCorridorComponent a c :=
  SameCorridorComponent.trans
    (SameCorridorComponent.overlap ab)
    (SameCorridorComponent.overlap bc)

/-- A bounded component-sort implementation is canonical once it establishes
that it preserves every input root and produces a list sorted by an
antisymmetric total key. Unlike the old manifest theorem, canonicality is a
conclusion here rather than an assumption repeated in the result. -/
theorem q_component_sort_canonical
    (sort : List QRootModel → List QRootModel)
    (le : QRootModel → QRootModel → Prop)
    (antisymmetric : ∀ a b, le a b → le b a → a = b)
    (permutation : ∀ roots, (sort roots).Perm roots)
    (sorted : ∀ roots, (sort roots).Pairwise le) :
    CanonicalComponentSort sort := by
  intro left right _ _ inputPermutation
  apply List.Perm.eq_of_pairwise
  · intro a b _ _ hab hba
    exact antisymmetric a b hab hba
  · exact sorted left
  · exact sorted right
  · exact (permutation left).trans
      (inputPermutation.trans (permutation right).symm)

end Pixels
