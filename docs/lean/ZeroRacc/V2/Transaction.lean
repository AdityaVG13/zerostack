import ZeroRacc.Foundations.Digests

namespace ZeroRacc.V2

structure DurableTransition (State Result : Type) where
  before : State
  result : Result
  after : State
  receipt : ZeroRacc.Foundations.ReceiptIdentity

/-- Exact serialization preserves a deterministic durable transition. -/
theorem exactStateResumption
    {State Action Result : Type}
    (step : State → Action → State × Result)
    (encode : State → State)
    (exact : ∀ state, encode state = state)
    (state : State) (action : Action) :
    step (encode state) action = step state action := by
  rw [exact state]

end ZeroRacc.V2
