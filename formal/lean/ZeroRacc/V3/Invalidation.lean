namespace ZeroRacc.V3

universe u v

def Changed {Artifact : Type u} {Value : Type v}
    (before after : Artifact → Value) : Artifact → Prop :=
  fun artifact => before artifact ≠ after artifact

/-- Every sound invalidation contains the exact extensional changed set. -/
theorem changedMinimal
    {Artifact : Type u} {Value : Type v}
    (before after : Artifact → Value)
    (invalidate : Artifact → Prop)
    (sound : ∀ artifact, before artifact ≠ after artifact → invalidate artifact) :
    ∀ artifact, Changed before after artifact → invalidate artifact := by
  intro artifact hchanged
  exact sound artifact hchanged

end ZeroRacc.V3
