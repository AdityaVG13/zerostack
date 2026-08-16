namespace ZeroRacc.Foundations

/-- A Boolean decision cannot equal both conflicting required actions. -/
theorem twoWorldCollision (a : Bool) : ¬ (a = false ∧ a = true) := by
  cases a <;> simp

/-- The unit code is a concrete non-injective compression witness. -/
def mergedBoolCode (_x : Bool) : Unit := ()

theorem mergedBoolCollision : mergedBoolCode false = mergedBoolCode true := by
  rfl

end ZeroRacc.Foundations
