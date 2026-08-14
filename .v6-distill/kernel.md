# V6-R11 bead report: conformance + contract completion (kernel slice)

Bead: `zerostack-bme7` (V6-R11). Covers ZS-KERNEL-001/007, ZS-CONTRACT-002,
ZS-BASE-001/003. Baseline: `racc/v6/distilled/xwalk_kernel.md` +
`.v6-distill/kernel.md` (pre-wave).

## ZS-KERNEL-001 -- canonical serialization / object-class registry

- `ObjectClassV1` extended from 8 to 12 classes: `DecisionView`
  (zerostack.object.decision_view.v1), `Delta`
  (zerostack.object.delta.v1), `AuthorityObject`
  (zerostack.object.authority_object.v1), `MigrationReceipt`
  (zerostack.object.migration_receipt.v1). Existing domains/roots unchanged
  (additive).
- Property breadth: canonical bytes are invariant under key insertion order,
  whitespace, and equivalent escape spellings; NFC/NFD and path-alias
  spellings never collide (byte-exact, no silent normalization).
- Tests: `tests/rust/zero-abi/unit/identity.rs`
  (`object_class_registry_covers_views_deltas_and_authority_objects`,
  `canonical_bytes_are_stable_across_perturbations_and_exact_otherwise`),
  `tests/rust/shared/rooted_abi_golden.rs`.

## ZS-KERNEL-007 -- versioned semantic ABI

- Cross-release golden fixture `conformance/fixtures/rooted_abi_golden_v6.json`:
  all 12 classes pinned to canonical bytes + roots; recomputation must match
  (decode-identically across releases).
- Migration machinery: `RootedAbiMigrationReceiptV1` -- pins legacy root
  (re-derived from the legacy preimage with the legacy ABI tag), v6 target
  root (canonical path only), real ABI change, nonempty reason; fails closed
  on forged source root, swapped bytes, no-op migration, legacy target ABI.
  The receipt roots under `MigrationReceipt` like any other class.
- Tests: `tests/rust/shared/rooted_abi_golden.rs`,
  `tests/rust/zero-abi/unit/identity.rs`
  (`migration_receipt_mints_verifies_and_fails_closed`).

## ZS-CONTRACT-002 -- model invocation contract

- `ReasoningContractV1` gains typed invocation bindings: `SamplingParamsV1`
  (temperature/top-p ppm + optional seed -- integer encoding, no float
  canonicalization hazards), `StoppingPolicyV1` (bounded stop sequences, max
  steps), `system_prompt_root` (non-zero digest when set), per-tool
  `ToolPermissionV1` map (read-only / approval-required / max calls, bounded
  256 tools). All four participate in canonical bytes, identity digest, and
  strict paired comparison (`InvocationBindingMismatch` on any change).
- Canonical schema updated in lockstep
  (`conformance/schemas/reasoning-contract-v1.schema.json` +
  `tests/contracts/` mirror); contract/schema digests re-pinned; downstream
  digest pins (assembly ABI digest, raw-worker protocol digest, reinvestment,
  deoptimization, invalidation, q99, two-phase v2/v3/v4/v5, program-assembly
  models) re-pinned to the landed contract -- fixtures updated, code not
  weakened.
- Tests: `tests/rust/zero-abi/unit/reasoning.rs`
  (`invocation_bindings_participate_in_identity_and_round_trip`,
  `invocation_binding_changes_reclassify_strict_pairs`,
  `invalid_invocation_bindings_fail_closed`).

## ZS-BASE-001 -- native path preservation

- Acceptance test: `tests/rust/zsx-core/native_path_survives_adapter_disable.rs`
  (feature `fixture-adapters`): a conformance task disables the fs adapter
  mid-run (engine_unavailable after the first call); the plan's later steps
  (including a loud adapter refusal handled by the plan) complete through
  native interpreter ops only; the session stays healthy for subsequent
  legacy and V6 requests; a plan without a native fallback fails loudly.

## ZS-BASE-003 -- fallback reserve

- Acceptance test: `tests/rust/zero-gate/unit/reinvestment.rs`
  (`injected_late_failure_is_absorbed_by_reserve_or_refused`): a late failure
  within reserve+slack stays `WithinRawBaseline` (slack unchanged); a late
  failure beyond reserve with no declared budget is refused
  (`BudgetExceeded`); with declared budget it is labeled
  `DeclaredAdditionalBudget`; injection into the stored record is a loud
  record-level refusal (accounting + digest checks).

## Note on concurrent work

`crates/zero-abi/src/{exec_dag,exec_trace,exec_stream}.rs` and
`tests/zero-testkit/src/bench_exec.rs` are R15's in-flight WIP (untracked,
not committed by this bead). Zero-abi compiles with them; my lib.rs hunks
were staged selectively so R15's hunks/files remain untouched in the tree.
