import Mathlib

namespace Pixels

structure OpenRunInvariant (State : Type) where
  structuralIdentity : State → State → Prop
  qOrder : State → State → Prop

def SameStrictSign (a b : ℝ) : Prop :=
  (a < 0 ∧ b < 0) ∨ (0 < a ∧ 0 < b)

/-- Every predicate active for a subject stays nonzero at every state in the
open run. `onOpenRun` includes the endpoint limits used by the runtime. -/
def ActivePredicatesNonzero
    {Subject State Predicate : Type}
    (active : Subject → Predicate → Prop)
    (value : Predicate → State → ℝ)
    (onOpenRun : Subject → State → State → State → Prop)
    (subject : Subject) (before after : State) : Prop :=
  ∀ predicate, active subject predicate →
    ∀ state, onOpenRun subject before after state →
      value predicate state ≠ 0

/-- A finite subject is complete when it is emitted or has an exclusion proof.
Open runs carry no zero of an active event predicate. Continuity converts that
non-vanishing premise to strict sign persistence, and the compiler's
feature/competition interpretation lemmas then derive stable structural
identity and q order. The theorem does not assume that identity or order
changes are detected as a separate oracle. -/
theorem conditionalEventCover
    {Subject State Predicate : Type}
    (emitted excluded changes : Subject → Prop)
    (openRun : Subject → State → State → Prop)
    (active : Subject → Predicate → Prop)
    (value : Predicate → State → ℝ)
    (onOpenRun : Subject → State → State → State → Prop)
    (invariant : OpenRunInvariant State)
    (accounted : ∀ subject, emitted subject ∨ excluded subject)
    (emittedCover : ∀ subject, changes subject → emitted subject →
      ∃ endpoint, endpoint = subject)
    (excludedAbsent : ∀ subject, excluded subject → ¬ changes subject)
    (openRunNoActiveZero : ∀ subject before after,
      openRun subject before after →
      ActivePredicatesNonzero active value onOpenRun subject before after)
    (signPersistence : ∀ subject before after,
      openRun subject before after →
      ActivePredicatesNonzero active value onOpenRun subject before after →
      ∀ predicate, active subject predicate →
        SameStrictSign (value predicate before) (value predicate after))
    (identityFromPredicateSigns : ∀ subject before after,
      openRun subject before after →
      (∀ predicate, active subject predicate →
        SameStrictSign (value predicate before) (value predicate after)) →
      invariant.structuralIdentity before after)
    (orderFromPredicateSigns : ∀ subject before after,
      openRun subject before after →
      (∀ predicate, active subject predicate →
        SameStrictSign (value predicate before) (value predicate after)) →
      invariant.qOrder before after) :
    (∀ subject, changes subject → emitted subject) ∧
    (∀ subject before after, openRun subject before after →
      invariant.structuralIdentity before after ∧ invariant.qOrder before after) := by
  constructor
  · intro subject hchange
    rcases accounted subject with hemitted | hexcluded
    · rcases emittedCover subject hchange hemitted with ⟨_, rfl⟩
      exact hemitted
    · exact False.elim ((excludedAbsent subject hexcluded) hchange)
  · intro subject before after hopen
    have hnonzero := openRunNoActiveZero subject before after hopen
    have hsigns := signPersistence subject before after hopen hnonzero
    exact ⟨
      identityFromPredicateSigns subject before after hopen hsigns,
      orderFromPredicateSigns subject before after hopen hsigns
    ⟩

end Pixels
