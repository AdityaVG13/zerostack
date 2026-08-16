namespace ZeroRacc.V3

structure QueryValue where
  protectedGain : Int
  workCost : Nat
  deriving DecidableEq, Repr

/-- A zero-gain positive-cost query is not strictly useful in this exact gauge. -/
theorem zeroGainNotPositive
    (query : QueryValue)
    (_positiveCost : 0 < query.workCost)
    (zeroGain : query.protectedGain = 0) :
    ¬ query.protectedGain > 0 := by
  simp [zeroGain]

end ZeroRacc.V3
