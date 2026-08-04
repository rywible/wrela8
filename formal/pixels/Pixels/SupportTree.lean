import Pixels.SmoothObject

namespace Pixels

inductive SupportTree where
  | leaf (id : Nat)
  | blend (budget : ℚ) (left right : SupportTree)

def SupportTree.maxBudget : SupportTree → ℚ
  | .leaf _ => 0
  | .blend budget left right =>
      max left.maxBudget right.maxBudget + budget

theorem SupportTree.childBudget_le
    (budget : ℚ) (left right : SupportTree) (hbudget : 0 ≤ budget) :
    left.maxBudget ≤
      (SupportTree.blend budget left right).maxBudget := by
  simp only [maxBudget]
  linarith [le_max_left left.maxBudget right.maxBudget]

end Pixels
