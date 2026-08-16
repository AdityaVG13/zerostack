import ZeroRacc.Foundations.Digests

namespace ZeroRacc.V3

structure ProjectImage where
  sourceRoot : ZeroRacc.Foundations.ReceiptIdentity
  fsRoot : ZeroRacc.Foundations.ReceiptIdentity
  graphRoot : ZeroRacc.Foundations.ReceiptIdentity
  tokenRoot : ZeroRacc.Foundations.ReceiptIdentity
  contractRoot : ZeroRacc.Foundations.ReceiptIdentity
  deriving DecidableEq, Repr

end ZeroRacc.V3
