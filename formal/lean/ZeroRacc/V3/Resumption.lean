namespace ZeroRacc.V3

universe u v w

/-- One-shot and resident execution agree under complete state serialization. -/
theorem oneShotResumption
    {State : Type u} {Action : Type v} {Result : Type w}
    (step : State → Action → State × Result)
    (encode : State → State)
    (exact : ∀ state, encode state = state)
    (state : State) (action : Action) :
    step (encode state) action = step state action := by
  rw [exact state]

end ZeroRacc.V3
