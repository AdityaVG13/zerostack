import ZeroRacc.Foundations.Finite

namespace ZeroRacc.RaccR

/-- A concrete non-injective compression merges conflicting Boolean states. -/
theorem boolCompressionCollision :
    ZeroRacc.Foundations.mergedBoolCode false =
      ZeroRacc.Foundations.mergedBoolCode true := by
  rfl

/-- An injective encoding alone does not force fixed-model invariance. -/
def swapCode : Bool → Bool
  | false => true
  | true => false

theorem swapCodeInjective : Function.Injective swapCode := by
  intro a b h
  cases a <;> cases b <;> simp [swapCode] at h ⊢

theorem fixedModelCanReverseEncodedAction (x : Bool) : swapCode x = !x := by
  cases x <;> rfl

end ZeroRacc.RaccR
