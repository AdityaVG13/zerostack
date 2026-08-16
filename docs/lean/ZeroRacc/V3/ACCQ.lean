import ZeroRacc.V3.PPCS

namespace ZeroRacc.V3

universe u v

theorem protectedContinuationRefl
    {History : Type u} {Continuation Trace : Type v}
    (trace : History → Continuation → Trace) (history : History) :
    ProtectedContinuationEquivalent trace history history := by
  intro continuation
  rfl

end ZeroRacc.V3
