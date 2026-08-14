# V6 Adapter Conformance Contract (V6-R11)

Schema version: `zerostack.v6_adapter_conformance.v1`
Applies to: every ZeroStack adapter (CLI / RPC / native addon / MCP) and every
engine adapter that serves the V6 zero-execute surface.

This contract is the engine-facing pass/refuse rule set each adapter must
satisfy. The hub runs it as fixture-driven conformance tests against fixture
transports (see `tests/zero-testkit/src/v6_conformance.rs`,
`tests/rust/shared/v6_cross_transport.rs`, `tests/rust/shared/v6_schema_identity.rs`,
`tests/rust/shared/task_contract_roundtrip.rs`, `tests/rust/shared/rooted_abi_golden.rs`);
engines run the same vectors against their own transports in their own repos.

## 1. Stable Zero Execute outer surface (ZS-ADAPTER-001/010)

- The registered tool definition (the schema document) must be byte-identical
  across 10,000 randomized tasks; dynamic project details live in rooted
  arguments/roots, never in schema mutation.
- Pass: every randomized request validates against
  `racc/v6/schemas/zero_execute_request_v6.schema.json`, and the instance
  shape (key sets per level, scalar leaves) is identical for every task.
- Refuse loudly: a request that mutates the envelope field set, reorders
  fields, or adds an unknown field.
- Fixtures: `racc/v6/schemas/zero_execute_request_v6.schema.json`
  (canonical-bytes pin in `tests/rust/shared/v6_schema_identity.rs`),
  `tests/fixtures/v6_cross_transport_vectors.json` (local mirror: `conformance/fixtures/`).

## 2. Cross-transport semantic replay (ZS-ADAPTER-009/011)

- The same canonical V6 vector must replay through at least three of the four
  transports (CLI JSON, raw-worker-v2 RPC frame, native addon envelope, MCP
  JSON-RPC) with byte-identical protected fields: `abi_version`, `kind`,
  `project_root`, `resource_ledger_root`, `continuation_handle`,
  `audit_event_range`.
- Equivalent semantics must survive: cancellation and timeout vectors stay
  `Cancelled`; Unknown-carrying vectors stay `VerificationUnknown` /
  `BaselineFallbackRequired` / `EvidenceExpansionRequired` with their
  reasons; fallback vectors stay `BaselineFallbackRequired`; every vector
  keeps its ledger root.
- Pass: recovery from every transport yields the identical typed envelope
  (`ZeroExecuteResultV6` equality) and identical protected fields; the six
  base kinds validate against
  `racc/v6/schemas/zero_execute_result_v6.schema.json`.
- Refuse loudly: relabeled kinds, swapped ledger roots, and injected unknown
  fields must fail recovery/validation or at minimum never be accepted as the
  original outcome (never laundered).
- Fixtures: `tests/fixtures/v6_cross_transport_vectors.json` (local mirror: `conformance/fixtures/`).

## 3. Task-contract round trip (ZS-ADAPTER-002)

- The canonical structured task contract (all fields) must round-trip through
  every transport with every semantic field byte-identical and one identical
  rooted contract (`StructuredTaskContractV1::contract_root`).
- Pass: the recovered contract equals the canonical contract and its root
  equals the fixture pin `tests/fixtures/task_contract_roundtrip_v6.json`
  (`expected_contract_root`).
- Refuse loudly: any tampered projection (mutated acceptance criterion,
  swapped contract digest) yields a different root and never verifies against
  the pinned root.

## 4. Rooted ABI golden registry (ZS-KERNEL-001/007)

- Every object class (including decision views, deltas, authority objects,
  and migration receipts) roots through the ONE canonical byte path:
  `canonical_object_bytes` + `object_root` under `zerostack.racc.v6`.
- Pass: all twelve classes re-derive their pinned canonical bytes and roots
  from `tests/fixtures/rooted_abi_golden_v6.json`.
- Incompatible-version migration is explicit and receipted
  (`RootedAbiMigrationReceiptV1`): the legacy root re-derives from the legacy
  preimage, the v6 target roots through the canonical path, and any tamper
  (forged source root, swapped bytes, no ABI change, legacy target) fails
  closed.

## 5. Invocation contract completion (ZS-CONTRACT-002)

- `ReasoningContractV1` binds sampling parameters (temperature/top-p ppm,
  seed), the stopping policy (stop sequences, max steps), the system prompt
  root, and per-tool permissions (read-only, approval-required, max calls).
- Paired baseline/treatment manifests reject any mismatch of these fields
  (`InvocationBindingMismatch`), and invalid bindings (out-of-range sampling,
  zero ceilings, control-character stop sequences, zero system prompt root,
  oversized permission maps) fail construction.
- Schema: `conformance/schemas/reasoning-contract-v1.schema.json` (mirrored in
  `tests/contracts/`).

## 6. Native path preservation and fallback reserve (ZS-BASE-001/003)

- A conformance task that disables ZeroStack mid-run must complete through
  the same native tool path, and the session must stay healthy afterwards
  (test: `tests/rust/zsx-core/native_path_survives_adapter_disable.rs`).
- An injected late failure must be absorbed by the fallback reserve (still
  within the raw baseline), refused before it begins (no declared budget), or
  labeled as declared-additional-budget -- and a late failure injected into a
  stored record is a loud record-level refusal (test:
  `tests/rust/zero-gate/unit/reinvestment.rs`,
  `injected_late_failure_is_absorbed_by_reserve_or_refused`).
