import Mathlib

namespace Pixels

inductive CsgExpr where
  | leaf (id : Nat)
  | not (child : CsgExpr)
  | and (left right : CsgExpr)
  | or (left right : CsgExpr)

def CsgExpr.eval (state : Nat → Bool) : CsgExpr → Bool
  | .leaf id => state id
  | .not child => !(child.eval state)
  | .and left right => left.eval state && right.eval state
  | .or left right => left.eval state || right.eval state

def subtract (left right : CsgExpr) : CsgExpr :=
  .and left (.not right)

theorem subtract_eval (state : Nat → Bool) (left right : CsgExpr) :
    (subtract left right).eval state =
      (left.eval state && !(right.eval state)) := by
  rfl

def CsgExpr.force (target : Nat) (value : Bool) : CsgExpr → CsgExpr
  | .leaf id => if id = target then
      if value then .or (.leaf id) (.not (.leaf id))
      else .and (.leaf id) (.not (.leaf id))
    else .leaf id
  | .not child => .not (child.force target value)
  | .and left right => .and (left.force target value) (right.force target value)
  | .or left right => .or (left.force target value) (right.force target value)

theorem CsgExpr.force_eval
    (expr : CsgExpr) (state : Nat → Bool) (target : Nat) (value : Bool) :
    (expr.force target value).eval state =
      expr.eval (fun id => if id = target then value else state id) := by
  induction expr with
  | leaf id =>
      by_cases h : id = target
      · cases value <;> simp [force, eval, h]
      · simp [force, eval, h]
  | not child ih =>
      simp [force, eval, ih]
  | and left right ihLeft ihRight =>
      simp [force, eval, ihLeft, ihRight]
  | or left right ihLeft ihRight =>
      simp [force, eval, ihLeft, ihRight]

inductive CrossingOrientation where
  | enter
  | exit
  deriving DecidableEq

def orientedToggleModel
    (inside : Bool) (orientation : CrossingOrientation) : Option Bool :=
  match inside, orientation with
  | false, .enter => some true
  | true, .exit => some false
  | _, _ => none

theorem csg_oriented_toggle_contract
    (inside : Bool) (orientation : CrossingOrientation) :
    orientedToggleModel inside orientation =
      match inside, orientation with
      | false, .enter => some true
      | true, .exit => some false
      | _, _ => none := by
  cases inside <;> cases orientation <;> rfl

def boundaryInfluencesModel (outside inside : Bool) : Bool :=
  outside != inside

theorem csg_boundary_influence_contract
    (outside inside : Bool) :
    boundaryInfluencesModel outside inside = true ↔ outside ≠ inside := by
  cases outside <;> cases inside <;> decide

def firstTransitionModel (initial : Bool) : List Bool → Option Nat
  | [] => none
  | value :: rest =>
      if value != initial then some 0
      else (firstTransitionModel initial rest).map Nat.succ

theorem csg_first_transition_contract
    (initial value : Bool) (rest : List Bool) :
    (value ≠ initial →
      firstTransitionModel initial (value :: rest) = some 0) ∧
    (value = initial →
      firstTransitionModel initial (value :: rest) =
        (firstTransitionModel initial rest).map Nat.succ) := by
  constructor
  · intro changed
    simp [firstTransitionModel, changed]
  · intro unchanged
    simp [firstTransitionModel, unchanged]

inductive CsgInst where
  | push (id : Nat)
  | not
  | and
  | or
  deriving DecidableEq

def CsgInst.exec (state : Nat → Bool) : CsgInst → List Bool → Option (List Bool)
  | .push id, stack => some (state id :: stack)
  | .not, value :: stack => some ((!value) :: stack)
  | .and, right :: left :: stack => some ((left && right) :: stack)
  | .or, right :: left :: stack => some ((left || right) :: stack)
  | _, _ => none

def CsgProgram.exec (state : Nat → Bool) : List CsgInst → List Bool → Option (List Bool)
  | [], stack => some stack
  | instruction :: rest, stack =>
      match instruction.exec state stack with
      | none => none
      | some next => CsgProgram.exec state rest next

def CsgExpr.compile : CsgExpr → List CsgInst
  | .leaf id => [.push id]
  | .not child => child.compile ++ [.not]
  | .and left right => left.compile ++ right.compile ++ [.and]
  | .or left right => left.compile ++ right.compile ++ [.or]

theorem CsgProgram.exec_append
    (state : Nat → Bool) (left right : List CsgInst) (stack : List Bool) :
    CsgProgram.exec state (left ++ right) stack =
      match CsgProgram.exec state left stack with
      | none => none
      | some next => CsgProgram.exec state right next := by
  induction left generalizing stack with
  | nil => rfl
  | cons instruction rest ih =>
      simp only [List.cons_append, CsgProgram.exec]
      split <;> simp_all

theorem CsgExpr.compile_exec
    (expr : CsgExpr) (state : Nat → Bool) (stack : List Bool) :
    CsgProgram.exec state expr.compile stack =
      some (expr.eval state :: stack) := by
  induction expr generalizing stack with
  | leaf id => rfl
  | not child ih =>
      rw [compile, CsgProgram.exec_append, ih]
      rfl
  | and left right ihLeft ihRight =>
      rw [compile, CsgProgram.exec_append, CsgProgram.exec_append, ihLeft]
      simp only
      rw [ihRight]
      rfl
  | or left right ihLeft ihRight =>
      rw [compile, CsgProgram.exec_append, CsgProgram.exec_append, ihLeft]
      simp only
      rw [ihRight]
      rfl

theorem CsgExpr.compiled_program_correct
    (expr : CsgExpr) (state : Nat → Bool) :
    CsgProgram.exec state expr.compile [] = some [expr.eval state] :=
  expr.compile_exec state []

end Pixels
