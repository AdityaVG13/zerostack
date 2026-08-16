namespace ZeroRacc.Foundations

universe u
variable {Q : Type u} [LE Q]

/-- A sound candidate guard publishes an outcome no worse than baseline. -/
theorem guardedChoiceDominates
    (baseline candidate : Q)
    (certificate : Prop)
    [Decidable certificate]
    (reflexive : ∀ quality : Q, quality ≤ quality)
    (sound : certificate → baseline ≤ candidate) :
    baseline ≤ (if certificate then candidate else baseline) := by
  by_cases h : certificate
  · simpa [h] using sound h
  · simpa [h] using reflexive baseline

end ZeroRacc.Foundations
