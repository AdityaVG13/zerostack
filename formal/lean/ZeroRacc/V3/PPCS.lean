namespace ZeroRacc.V3

universe u v

/-- Histories agree when every protected continuation has the same trace. -/
def ProtectedContinuationEquivalent
    {History : Type u} {Continuation Trace : Type v}
    (trace : History → Continuation → Trace) (a b : History) : Prop :=
  ∀ continuation, trace a continuation = trace b continuation

end ZeroRacc.V3
