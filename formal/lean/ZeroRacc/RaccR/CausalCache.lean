namespace ZeroRacc.RaccR

structure WorkBreakdown where
  baseline : Nat
  residual : Nat
  deriving DecidableEq, Repr

def q99Total (work : WorkBreakdown) : Prop :=
  100 * work.residual ≤ work.baseline

structure CacheCertificate where
  sourceExact : Bool
  producerExact : Bool
  dependenciesComplete : Bool
  protectedUseBound : Bool
  reasoningBound : Bool
  verifierBound : Bool
  deriving DecidableEq, Repr

def strictReusable (certificate : CacheCertificate) : Bool :=
  certificate.sourceExact && certificate.producerExact &&
  certificate.dependenciesComplete && certificate.protectedUseBound &&
  certificate.reasoningBound && certificate.verifierBound

end ZeroRacc.RaccR
