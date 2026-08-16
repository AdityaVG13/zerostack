namespace ZeroRacc.Foundations

structure ResourceVec where
  steps : Nat
  bytesRead : Nat
  arenaBytes : Nat
  outputBytes : Nat
  workerCalls : Nat
  processes : Nat
  deadlineTicks : Nat
  deriving DecidableEq, Repr

def ResourceVec.le (a b : ResourceVec) : Prop :=
  a.steps ≤ b.steps ∧
  a.bytesRead ≤ b.bytesRead ∧
  a.arenaBytes ≤ b.arenaBytes ∧
  a.outputBytes ≤ b.outputBytes ∧
  a.workerCalls ≤ b.workerCalls ∧
  a.processes ≤ b.processes ∧
  a.deadlineTicks ≤ b.deadlineTicks

instance : LE ResourceVec where le := ResourceVec.le

theorem resourceLeRefl (a : ResourceVec) : a ≤ a := by
  exact ⟨Nat.le_refl _, Nat.le_refl _, Nat.le_refl _, Nat.le_refl _,
    Nat.le_refl _, Nat.le_refl _, Nat.le_refl _⟩

theorem resourceLeTrans {a b c : ResourceVec} (hab : a ≤ b) (hbc : b ≤ c) : a ≤ c := by
  rcases hab with ⟨h1, h2, h3, h4, h5, h6, h7⟩
  rcases hbc with ⟨k1, k2, k3, k4, k5, k6, k7⟩
  exact ⟨Nat.le_trans h1 k1, Nat.le_trans h2 k2, Nat.le_trans h3 k3,
    Nat.le_trans h4 k4, Nat.le_trans h5 k5, Nat.le_trans h6 k6,
    Nat.le_trans h7 k7⟩

end ZeroRacc.Foundations
