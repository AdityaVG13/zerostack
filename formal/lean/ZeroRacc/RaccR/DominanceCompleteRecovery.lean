namespace ZeroRacc.RaccR

universe u v

def CommonSafe {World : Type u} {Effect : Type v}
    (safe : World → Effect → Prop) (fiber : List World) (effect : Effect) : Prop :=
  ∀ world ∈ fiber, safe world effect

/-- Deterministic recovery exists exactly when the fiber has a common-safe effect. -/
theorem dominanceCompleteRecoveryIff
    {World : Type u} {Effect : Type v}
    (safe : World → Effect → Prop) (fiber : List World) :
    (∃ choose : Unit → Effect, ∀ world ∈ fiber, safe world (choose ())) ↔
      ∃ effect, CommonSafe safe fiber effect := by
  constructor
  · rintro ⟨choose, hsafe⟩
    exact ⟨choose (), hsafe⟩
  · rintro ⟨effect, hsafe⟩
    exact ⟨fun _ => effect, hsafe⟩

end ZeroRacc.RaccR
