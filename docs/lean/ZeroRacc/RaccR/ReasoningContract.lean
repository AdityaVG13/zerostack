import ZeroRacc.Foundations.Digests

namespace ZeroRacc.RaccR

structure ReasoningContract where
  model : ZeroRacc.Foundations.ReceiptIdentity
  effort : Nat
  outputLimit : Nat
  tools : List String
  deriving DecidableEq, Repr

/-- Strict execution preserves the frozen reasoning contract exactly. -/
def StrictExecution (frozen actual : ReasoningContract) : Prop := actual = frozen

theorem strictExecutionNoDownshift
    (frozen actual : ReasoningContract)
    (strict : StrictExecution frozen actual) :
    actual.model = frozen.model ∧
    actual.effort = frozen.effort ∧
    actual.outputLimit = frozen.outputLimit ∧
    actual.tools = frozen.tools := by
  subst actual
  exact ⟨rfl, rfl, rfl, rfl⟩

end ZeroRacc.RaccR
