namespace ZeroRacc.Conjectures

/-- Data recorded by experiments for the prepared-repository conjecture track. -/
structure PreparedRepositoryPhase where
  projectSize : Nat
  taskCount : Nat
  exactReusePermille : Nat
  certifiedRescueCount : Nat
  deriving Repr

end ZeroRacc.Conjectures
