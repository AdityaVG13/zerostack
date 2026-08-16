import ZeroRacc.Foundations.Digests

namespace ZeroRacc.V2

inductive EffectClass where
  | readOnly
  | reversible
  | publish
  deriving DecidableEq, Repr

structure Effect where
  effectClass : EffectClass
  payload : ByteArray
  deriving DecidableEq, Repr

end ZeroRacc.V2
