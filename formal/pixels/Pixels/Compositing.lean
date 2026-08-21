import Mathlib

namespace Pixels

structure Transfer where
  color : ℝ
  transmittance : ℝ

def Transfer.compose (front back : Transfer) : Transfer where
  color := front.color + front.transmittance * back.color
  transmittance := front.transmittance * back.transmittance

theorem transfer_compose_associative (a b c : Transfer) :
    (a.compose b).compose c = a.compose (b.compose c) := by
  cases a
  cases b
  cases c
  simp [Transfer.compose]
  constructor <;> ring

def Transfer.identity : Transfer where
  color := 0
  transmittance := 1

theorem transfer_identity_left (value : Transfer) :
    Transfer.identity.compose value = value := by
  cases value
  simp [Transfer.identity, Transfer.compose]

theorem transfer_identity_right (value : Transfer) :
    value.compose Transfer.identity = value := by
  cases value
  simp [Transfer.identity, Transfer.compose]

def transferBalanced : List Transfer → Transfer
  | [] => Transfer.identity
  | value :: rest => value.compose (transferBalanced rest)

theorem transfer_balanced_model
    (front : Transfer) (rest : List Transfer) :
    transferBalanced (front :: rest) =
      front.compose (transferBalanced rest) := by
  rfl

def transferReplace
    (values : List Transfer) (index : Nat) (replacement : Transfer) :
    List Transfer :=
  values.set index replacement

theorem transfer_replace_model
    (values : List Transfer) (index : Nat) (replacement : Transfer) :
    transferBalanced (transferReplace values index replacement) =
      transferBalanced (values.set index replacement) := by
  rfl

theorem transfer_replace_preserves_other_leaf
    (values : List Transfer) (index other : Nat) (replacement : Transfer)
    (different : other ≠ index) :
    (transferReplace values index replacement)[other]? = values[other]? := by
  simp [transferReplace, List.getElem?_set, Ne.symm different]

end Pixels
