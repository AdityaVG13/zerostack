import ZeroRacc.Foundations.Digests
import ZeroRacc.V2.Effect

namespace ZeroRacc.V2

structure Witness where
  effect : Effect
  verifier : ZeroRacc.Foundations.ReceiptIdentity
  evidence : ZeroRacc.Foundations.ReceiptIdentity
  deriving DecidableEq, Repr

end ZeroRacc.V2
