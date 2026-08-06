namespace ZeroRacc.RaccR

structure Outcome where
  quality : Int
  work : Nat
  deriving DecidableEq, Repr

def WeaklyDominates (candidate baseline : Outcome) : Prop :=
  candidate.quality ≥ baseline.quality ∧ candidate.work ≤ baseline.work

def StrictlyParetoDominates (candidate baseline : Outcome) : Prop :=
  WeaklyDominates candidate baseline ∧
    (candidate.quality > baseline.quality ∨ candidate.work < baseline.work)

theorem qualityUpWorkDownIsStrict
    (candidate baseline : Outcome)
    (quality : candidate.quality > baseline.quality)
    (work : candidate.work < baseline.work) :
    StrictlyParetoDominates candidate baseline := by
  constructor
  · exact ⟨Int.le_of_lt quality, Nat.le_of_lt work⟩
  · exact Or.inl quality

end ZeroRacc.RaccR
