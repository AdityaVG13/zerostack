namespace ZeroRacc.V3

inductive ShieldDecision where
  | admit
  | restoreBaseline
  deriving DecidableEq, Repr

/-- A fail-closed shield always chooses admission or baseline restoration. -/
theorem shieldDecisionExhaustive (decision : ShieldDecision) :
    decision = .admit ∨ decision = .restoreBaseline := by
  cases decision <;> simp

end ZeroRacc.V3
