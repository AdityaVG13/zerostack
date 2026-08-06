namespace ZeroRacc.RaccR

/-- Complete pathwise candidate-first work without truncated subtraction. -/
def candidateFirstWork (accepted : Bool)
    (candidate admission deopt baseline : Nat) : Nat :=
  candidate + admission + (if accepted then 0 else deopt + baseline)

theorem acceptedWork (candidate admission deopt baseline : Nat) :
    candidateFirstWork true candidate admission deopt baseline =
      candidate + admission := by
  simp [candidateFirstWork]

theorem rejectedWork (candidate admission deopt baseline : Nat) :
    candidateFirstWork false candidate admission deopt baseline =
      candidate + admission + (deopt + baseline) := by
  simp [candidateFirstWork]

end ZeroRacc.RaccR
