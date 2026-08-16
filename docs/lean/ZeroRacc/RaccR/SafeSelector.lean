import ZeroRacc.RaccR.DominanceCompleteRecovery

namespace ZeroRacc.RaccR

universe u v

structure SafeSelection {World : Type u} {Effect : Type v}
    (safe : World → Effect → Prop) (fiber : List World) where
  effect : Effect
  proof : CommonSafe safe fiber effect

/-- A common-safe witness constructs a deterministic safe selector. -/
def safeSelectorOfCommon
    {World : Type u} {Effect : Type v}
    (safe : World → Effect → Prop) (fiber : List World) (effect : Effect)
    (proof : CommonSafe safe fiber effect) : SafeSelection safe fiber :=
  ⟨effect, proof⟩

end ZeroRacc.RaccR
