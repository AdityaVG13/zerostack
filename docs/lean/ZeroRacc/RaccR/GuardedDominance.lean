import ZeroRacc.Foundations.Orders

namespace ZeroRacc.RaccR

universe u
variable {Q : Type u} [LE Q]

/-- Failed admission deoptimizes exactly to the frozen raw baseline. -/
def publish (certificate : Bool) (candidate baseline : Q) : Q :=
  if certificate then candidate else baseline

theorem guardedBaselineDominance
    (certificate : Bool) (candidate baseline : Q)
    (reflexive : ∀ quality : Q, quality ≤ quality)
    (sound : certificate = true → baseline ≤ candidate) :
    baseline ≤ publish certificate candidate baseline := by
  cases certificate
  · simpa [publish] using reflexive baseline
  · simpa [publish] using sound rfl

end ZeroRacc.RaccR
