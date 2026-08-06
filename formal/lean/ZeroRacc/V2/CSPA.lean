namespace ZeroRacc.V2

/-- A certified branch and the raw-baseline branch exhaust a Boolean guard. -/
theorem guardExhaustive (certificate : Bool) :
    certificate = true ∨ certificate = false := by
  cases certificate <;> simp

end ZeroRacc.V2
