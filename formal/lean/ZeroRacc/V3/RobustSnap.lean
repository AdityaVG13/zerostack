namespace ZeroRacc.V3

universe u v

def CommonSafe {World : Type u} {Effect : Type v}
    (consistent : World → Prop) (safe : World → Effect → Prop) : Effect → Prop :=
  fun effect => ∀ world, consistent world → safe world effect

/-- Deterministic pointwise safety is equivalent to a common-safe effect. -/
theorem robustSnapCharacterization
    {World : Type u} {Effect : Type v}
    (consistent : World → Prop) (safe : World → Effect → Prop) :
    (∃ choose : Unit → Effect,
      ∀ world, consistent world → safe world (choose ())) ↔
    ∃ effect, CommonSafe consistent safe effect := by
  constructor
  · rintro ⟨choose, hsafe⟩
    exact ⟨choose (), hsafe⟩
  · rintro ⟨effect, hsafe⟩
    exact ⟨fun _ => effect, hsafe⟩

end ZeroRacc.V3
