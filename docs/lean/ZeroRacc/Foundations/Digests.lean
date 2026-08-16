namespace ZeroRacc.Foundations

deriving instance Repr for ByteArray

/-- An abstract receipt identity. Cryptographic collision resistance is external. -/
structure ReceiptIdentity where
  algorithm : String
  bytes : ByteArray
  deriving DecidableEq, Repr

/-- Systems premises carry the receipt that binds the asserted scope. -/
structure ReceiptedPremise (P : Prop) where
  receipt : ReceiptIdentity
  evidence : P

end ZeroRacc.Foundations
