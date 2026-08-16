import ZeroRacc.RaccR.CausalCache

namespace ZeroRacc.RaccR

/-- Exact finite Q99-Total feasibility in the integer work gauge. -/
theorem q99TotalIff (baseline residual : Nat) :
    q99Total ⟨baseline, residual⟩ ↔ 100 * residual ≤ baseline := by
  rfl

theorem q99ZeroResidual (baseline : Nat) :
    q99Total ⟨baseline, 0⟩ := by
  simp [q99Total]

end ZeroRacc.RaccR
