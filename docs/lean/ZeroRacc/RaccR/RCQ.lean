namespace ZeroRacc.RaccR

universe u v w

def SameContinuation {History : Type u} {Continuation : Type v} {Trace : Type w}
    (trace : History → Continuation → Trace) (a b : History) : Prop :=
  ∀ continuation, trace a continuation = trace b continuation

theorem sameContinuationRefl
    {History : Type u} {Continuation : Type v} {Trace : Type w}
    (trace : History → Continuation → Trace) (history : History) :
    SameContinuation trace history history := by
  intro continuation
  rfl

theorem sameContinuationTrans
    {History : Type u} {Continuation : Type v} {Trace : Type w}
    (trace : History → Continuation → Trace) {a b c : History}
    (hab : SameContinuation trace a b)
    (hbc : SameContinuation trace b c) : SameContinuation trace a c := by
  intro continuation
  exact Eq.trans (hab continuation) (hbc continuation)

end ZeroRacc.RaccR
