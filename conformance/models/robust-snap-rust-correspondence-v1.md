# Robust Snap v1 Rust correspondence plan

## Scope

This artifact maps the frozen finite model to shipped Rust types. It is not an operational compiler or execution proof.

- World fiber: `zero_abi::WorldFiberDescriptor`
- Per-world protected effects: `zero_abi::ProtectedEffectSet`
- S0/S1 witness: `zero_abi::RobustSnapCertificate`
- Evidence partition: `zero_abi::EvidenceDecisionTree`
- UNKNOWN/S0/S1 result: `zero_abi::SnapLevel`
- Heuristic non-narrowing check: `zero_abi::validate_heuristic_world_order`

## Correspondence obligations

1. Engine adapters must map complete operational worlds to the frozen finite fiber without omission.
2. Effect equality must preserve both digest and effect class.
3. Protected-effect and verifier membership must come from independent evidence, not ranking.
4. Evidence outcomes must map to one nonempty, disjoint, exhaustive leaf partition.
5. Assembly manifest and model version must remain bound in every certificate.

## Claim boundary

The Rust checker proves only membership and finite partition laws for supplied data. It does not prove that supplied worlds are operationally complete, that an effect compiler exists, or that execution is durable. `SnapLevel::permits_operational_execution()` is always false. Operational S1 remains bead `zerostack-racc-frontier-86qk.24`.
