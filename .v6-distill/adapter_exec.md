# V6-R11 bead report: conformance + contract completion (adapter/exec slice)

Bead: `zerostack-bme7` (V6-R11). Covers ZS-ADAPTER-001/002/009/010/011,
ZS-KERNEL-001/007, ZS-CONTRACT-002, ZS-BASE-001/003. Baseline audit:
`racc/v6/distilled/xwalk_adapter_exec.md` + `.v6-distill/adapter_exec.md`
(pre-wave). This report records what V6-R11 landed and how each row is now
evidenced by fixture-driven conformance tests.

## Rows closed by fixture-driven conformance surface

| Row | What landed (this bead) | Evidence |
|---|---|---|
| ZS-ADAPTER-001 (stable outer tool) | 10,000 randomized task schema-identity test: every randomized request validates against `zero_execute_request_v6.schema.json` and the instance shape (key sets per level, scalar leaves) is byte-identical; schema document hash pinned | `tests/rust/shared/v6_schema_identity.rs` (proptest 10k cases), `racc/v6/schemas/zero_execute_request_v6.schema.json` |
| ZS-ADAPTER-002 (structured submission) | Canonical task-contract cross-transport round-trip fixture: every semantic field survives CLI/RPC/native/MCP byte-identically with ONE identical rooted contract; tampered projections are different roots and refused | `conformance/fixtures/task_contract_roundtrip_v6.json` + `tests/rust/shared/task_contract_roundtrip.rs` |
| ZS-ADAPTER-009 (adapter conformance suite) | Golden cross-adapter suite: same canonical V6 vectors replayed through >=3 fixture transports with byte-identical protected fields (kind, project root, ledger root, continuation handle, audit range); violation vectors refused loudly | `conformance/fixtures/v6_cross_transport_vectors.json` + `tests/rust/shared/v6_cross_transport.rs` + `tests/zero-testkit/src/v6_conformance.rs` |
| ZS-ADAPTER-010 (stable surface) | Same 10k schema-identity test covers the request surface; envelope shape registry pinned per kind (8-kind union stable) | `tests/rust/shared/v6_schema_identity.rs` |
| ZS-ADAPTER-011 (cross-transport replay) | Semantic replay contract: cancellation/timeout stay Cancelled, Unknown kinds keep reasons, fallback stays BaselineFallbackRequired, ledger root identical across all transports | `v6_cross_transport` tests + `conformance/contracts/v6-adapter-conformance.md` |

## Engine-facing contract doc

`conformance/contracts/v6-adapter-conformance.md` -- the pass/refuse rule set
each adapter (CLI/RPC/native/MCP) must satisfy, referencing fixtures, harness,
and tests. Engines run the same vectors in their own repos.

## Harness

`tests/zero-testkit/src/v6_conformance.rs` (feature `full`): fixture
transports (CLI JSON, raw-worker-v2 NDJSON frame through the shared frame
codec, native addon envelope, MCP JSON-RPC), `envelope_from_vector`,
`protected_fields`, `apply_violation` (per-transport envelope targeting),
`shape_signature`.

## Residual risks

- RPC fixture transport is the raw-worker frame codec, not a live worker
  process; engines must re-run against their real transports.
- Violation recovery for kind relabeling on Cancelled->Completed relies on
  per-kind validation (fail-closed); schema-level rejection is only asserted
  for the six base kinds (the two adapter extensions are outside the schema
  enum by design).
