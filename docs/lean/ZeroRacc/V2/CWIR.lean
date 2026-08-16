import ZeroRacc.Foundations.Digests

namespace ZeroRacc.V2

structure StateAnchor where
  sourceRoot : ZeroRacc.Foundations.ReceiptIdentity
  toolchain : ZeroRacc.Foundations.ReceiptIdentity
  manifest : ZeroRacc.Foundations.ReceiptIdentity
  deriving DecidableEq, Repr

structure CWIR where
  taskContract : ZeroRacc.Foundations.ReceiptIdentity
  state : StateAnchor
  evidence : List ZeroRacc.Foundations.ReceiptIdentity
  verifierScope : ZeroRacc.Foundations.ReceiptIdentity
  deriving DecidableEq, Repr

end ZeroRacc.V2
