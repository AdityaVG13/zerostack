# ZeroRacc

This is the canonical Lean project for ZeroStack aggregate contracts. It pins
Lean to `v4.32.0`. The initial modules define the finite theorem
surface and explicit system-premise records. They do not attest Rust behavior,
provider state, filesystem durability, graph coverage, or verifier soundness.

Build the release surface:

```bash
lake update
lake build ZeroRacc
lake env lean ZeroRacc/AxiomReport.lean
python3 scripts/check_trust.py
```

A release attestation needs the source digest, pinned build log, and axiom
report. Conjecture modules are outside `ZeroRacc.All` and cannot authorize a
runtime guard.
