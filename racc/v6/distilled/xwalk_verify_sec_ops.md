# Crosswalk Audit: ZS-VERIFY-001..007, ZS-SEC-001..005, ZS-OPS-001..006, ZS-RELEASE-001

Audited against `/Users/aditya/AI/ZeroStack` (HEAD `48d71d3`), crates: zero-cert, zero-gate, zero-abi, zero-store, zero-process, zerostack-machine-permit, zsx-core, zsx-node, zero-codemode; conformance/. CSV statuses ("Not implemented / audit required") are corpus defaults; this audit reflects actual repo state.

**Verdict summary:** implemented 2 (ZS-SEC-003, ZS-OPS-001), partial 11, missing 6 (ZS-VERIFY-003, ZS-VERIFY-006, ZS-SEC-004, ZS-OPS-004, ZS-SEC-005, + ZS-VERIFY-001/002/004/007 partially missing fields). No `conflicting` found.

---

## ZS-VERIFY-001 — Verifier registry

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-abi/src/assembly.rs`: `VerifierIdentityV1` (line ~193: verifier_id/version/digest), `AssemblyManifestV1.verifiers` + `validate()` (canonical order, nonempty, nonblank; ~line 324). `crates/zero-cert/src/lib.rs`: `Provenance` (L113: parser/index/operator id+version), `OperatorLock` (L122), `Resolver::trusted_operator_version/trusted_parser_version/trusted_index_version` (L318), `VerificationError::{MissingTrustedOperator, StaleOperator, StaleParser, StaleIndex}` (L303), `verify()` (L358). `crates/zero-gate/src/lib.rs`: `TaskRunEvidence.verifier + verifier_environment_digest` (~L137), `TaskAcceptanceReceipt.verifier`. Tests: `tests/rust/zero-cert/golden_vectors.rs` `trusted_locks_reject_missing_and_stale_operator_parser_and_index` (L78), `rejects_cross_query_parameter_substitution_for_every_witness` (L176). |
| **GAP** | No runtime registry record binding verifier -> {input roots, result, evidence, runtime, confidence/assurance grade}. `input_token_cost`/`backend_work_units` exist on the certificate but there is no runtime_ms or confidence/assurance-grade field anywhere in crates/. Version-change invalidation is enforced (trusted-lock + digest binding), but a registry table with scope+result+grade is absent. |
| **CONFIDENCE** | high (enforcement half proven by tests; absence of grade/runtime fields confirmed by grep over zero-cert/zero-gate/zero-abi) |

## ZS-VERIFY-002 — Current-effect verification

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-gate/src/lib.rs`: `verify_task_acceptance` (~L480) checks exit_code, artifact digest equality (expected vs observed), `TaskAcceptanceVerifier::verify_run`; `begin_task_attempt` rejects irreversible/zero-cost/too-many-artifacts. `crates/zero-gate/src/two_phase.rs`: `ExecutionBinding` (L798: source_tree_digest, task_fingerprint_digest, plan_digest, state_snapshot_digest = sandbox root, image_digest), `candidate_protocol_identity_v1` (L843), `validate_g7` performance/quality admission (L1590). `crates/zero-cert/src/lib.rs`: `CompletenessWitness::BuildReceipt`/`TestTrace` (L159). Tests: `tests/rust/zero-gate/unit/two_phase.rs` `state_machine_strict_artifacts_omitted_guards_and_forged_predecessors_fail` (L770), `state_machine_admission_and_receipt_commitments_reject_tampering` (L988). |
| **GAP** | Delta identity is digest-bound (task fingerprint/plan/source tree), and build/test evidence has witnesses; but there is no obligation checklist for static-analysis/security scans, and no explicit check that verification ran against the exact candidate delta + sandbox root as one operation (no test for "substitute a different delta after verification"). |
| **CONFIDENCE** | medium-high (digest binding strong; missing explicit static/security obligation coverage) |

## ZS-VERIFY-003 — Successor-state verification

| | |
|---|---|
| **STATUS** | **missing** |
| **EVIDENCE** | Adjacent-only: `crates/zero-gate/src/two_phase.rs` `ReceiptCommon.successor_root`/`ReceiptRecord.successor_root` (L2740/L2750), `predecessor_receipt_head`; `crates/zero-gate/src/durable_publication.rs` L351-355 ("gate successor root differs from journal new root"); `crates/zero-gate/src/deoptimization.rs` `successor_receipt_head_digest` (L983). `crates/zero-gate/src/semantic_cut.rs` `ReasoningSafepointV1.open_obligations_digest` (L52). |
| **GAP** | No mechanism verifies that a committed state preserves registered future interfaces/invariants or a declared simulation relation. `successor_root` is a chaining digest, not a successor-state check. No fixture rejects a locally-passing edit that breaks a registered future action. |
| **CONFIDENCE** | high (successor concept exists only as root chaining; no successor-state verifier symbol) |

## ZS-VERIFY-004 — Equivalence/dominance verdict

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-gate/src/recovery.rs`: `RecoveryDecisionV1::{Complete, Conflict, Unknown}` (L887), `dominance_complete_recover_v1` (L895); module doc: "`Unknown` always requires frozen raw-baseline fallback" (L4-8). `crates/zero-gate/src/semantic_cut.rs`: `ReasoningStateStatusV1::{ExactPreserved, ExactCleanRestart, ScopedEquivalent, Approximate, Unavailable, Expired, IdentityMismatch}` (L32). `crates/zero-gate/src/two_phase.rs` `validate_g7` (L1590): `QualityEvidenceClassV1::{ExactNeutral, PointwiseDominance, ScopedClassDominance}` -> candidate, `{Distributional, Unidentified}` -> FrozenBaseline only. Tests: `tests/rust/zero-gate/unit/two_phase.rs` `state_machine_quality_envelope_guards_candidate_and_distributional_fallback` (L624). |
| **GAP** | No unified verdict type returning Equivalent/Dominates/Reject/Unknown for verification under protected scope; Unknown is never mapped from verifier timeout/disagreement/uncovered-dimension (no timeout path in zero-cert `verify`). "Unknown not promotable" is honored in DCR/quality paths, but there is no verdict object. |
| **CONFIDENCE** | high (vocabulary distributed across recovery/quality modules; no single verdict API) |

## ZS-VERIFY-005 — Authority lease issuance

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-gate/src/two_phase.rs`: `prepare()` (L1249) runs G0-G7 then issues opaque linear `ExecutionPermit` (L1196); `validate_g3` semantic-cut binds input/plan/model/reasoning contract/comparison/scope/verifier (L1499); `validate_g4` resource reserve envelope: fuel, deadline_ms, io/output/memory/processes/risk_units/worker_steps (L1527); `validate_g5` robust-snap verifier receipt (L1548); `validate_g6` safety shield + `approval_grant_digest` requirement for ApprovalRequiredMutation (L1590); `validate_g7` quality admission. `crates/zero-abi/src/raw_worker.rs`: `ApprovalGrant` (L257) + `validate_approval_grant` (L283: engine/root/session/request/operation binding, effect, issued<=expires, consumed-set). `crates/zsx-core/src/connector.rs`: `SessionApprovalGrantV1` (L69), `MAX_SESSION_APPROVAL_LIFETIME_MS=300_000` (L88), `install_approvals`/`take_approval` (L784/L807). `crates/zerostack-machine-permit/src/lib.rs`: `MachinePermit` (L371) slot acquisition + heartbeat liveness (L84). Tests: `tests/rust/zsx-core/unit/session.rs` `session_approval_contract_is_bounded_and_replay_safe` (L12); `tests/rust/zero-gate/unit/two_phase.rs` `state_machine_forged_permit_unbounded_worker_semantic_cut_and_image_fail` (L806). |
| **GAP** | Root/scope/epoch/delta/verifier-receipts/resource-reserve/expiry/nonce are all checked at issuance, but: (a) `ExecutionPermit` itself carries no expiry/epoch — only `envelope.deadline_ms` and grant expiry; (b) no explicit caller identity field in the two-phase gate (session_id appears only in worker-level grants); (c) no in-repo grant issuer — grants are host-supplied inputs; issuance authority is external. |
| **CONFIDENCE** | high (all pre-conditions verified in code+tests; caller identity and permit-expiry gaps confirmed) |

## ZS-VERIFY-006 — Human/user acceptance gate

| | |
|---|---|
| **STATUS** | **missing** |
| **EVIDENCE** | Zero occurrences of `DecisionRequired`, `UserApproval`, or a declared-evaluator representation across crates/. Closest: `EffectClass::ApprovalRequiredMutation` + approval grants (`crates/zero-abi/src/raw_worker.rs` L257; `crates/zsx-core/src/session.rs` `validate_session_approvals` L1496) gate mutations behind a host-supplied grant, and `crates/zero-gate/src/two_phase.rs` `validate_g6` (L1590) requires `approval_grant_digest`. |
| **GAP** | No explicit DecisionRequired/UserApproval verdict that stops authority for subjective/policy decisions; nothing prevents auto-claiming subjective superiority without a declared evaluator (no such claim path exists either — greenfield superiority claims are simply not representable). ZS-EXEC-007 (DecisionRequired gate) is a separate backlog item. |
| **CONFIDENCE** | high (grep across crates/ for the mandated vocabulary returned nothing; grant mechanism is the only approval lever) |

## ZS-VERIFY-007 — Proof/evidence cache

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-gate/src/q99.rs`: `CausalCacheBindingV1` (L160: artifact_digest, source_root, dependency_root, producer_contract_digest, protected_use_class_digest, reasoning_contract_digest, verifier_scope_digest, invalidation_certificate_digest, recovery_route_digest), `CacheCoordinateV1` (L44), `CacheValidityV1` (L85), `validate_causal_cache_v1` (L469), `VerifiedCausalWorkReceiptV1::verify` (L577). `crates/zero-gate/src/invalidation.rs`: `BoundCausalCacheInvalidationV1` (L635), `verify_robust_snap_intake_v1` (L227), `CausalArtifactIntakeClaimV1` (L365). Tests: `tests/rust/zero-gate/unit/q99.rs` `cache_authority_rejects_missing_components_and_unmatched_evidence` (L287), `aggregate_cache_validation_keeps_semantics_telemetry_and_reasoning_distinct` (L184); `tests/rust/zero-gate/unit/invalidation.rs` `record_tampering_and_noncanonical_replay_fail_closed` (L528). |
| **GAP** | Complete causal keys + invalidation binding exist as a pure validation contract; there is no live cache store keyed by these bindings with equality early cutoff, and no test proving "changed unrelated file preserves proof object". Code/test/tool/env changes invalidate only through the digest binding, not through a runtime invalidation engine. |
| **CONFIDENCE** | high (contract-level cache authority proven; runtime cache semantics absent) |

---

## ZS-SEC-001 — Sandbox confinement

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | No chroot/unshare/seccomp/landlock anywhere (grep confirmed). `crates/zero-process/src/child.rs`: `VerifiedChild` (L321) with `ProcessIdentity` capture/verify (`crates/zero-process/src/identity.rs` L77), owner-session+generation binding, revocation (L700), unix process-group isolation + Windows job-object kill-on-close (L360-375), `spawn_tree_with_pipes_and_policy` (L393). `crates/zero-process/src/resource.rs`: `ProcessResourcePolicy` (L11) + pre-exec `setrlimit` RLIMIT_AS/RLIMIT_CPU (L136-176; Darwin note: RLIMIT_AS not enforced). `crates/zero-codemode/src/worker.rs`: strips `ZEROSTACK_SESSION_TOKEN`/`SHUTDOWN_TOKEN` from child env (L391-392). Tests: `tests/rust/zero-process/child.rs` `owner_mismatch_is_rejected` (L223), `generation_mismatch_is_rejected` (L240), `verified_child_escalates_stubborn_child` (L314). |
| **GAP** | Filesystem namespace, network, device, environment (beyond two token vars), and secret policies are not confined. No adversarial symlink/path-traversal/socket/subprocess fixtures exist. Process-tree isolation + rlimits + identity is confinement only in the weakest sense. |
| **CONFIDENCE** | high (negative grep for OS confinement primitives; positive code for process isolation) |

## ZS-SEC-002 — Receipt and object poisoning defense

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-cert/src/lib.rs`: canonical-encoding verification (`EvidenceCertificate::canonical_digest` L275, `verify` L358, `VerificationError` L303), receipt roots via `Resolver::resolve_mutation_receipt`/`resolve_aggregate_receipt` (L318). `crates/zero-gate/src/invalidation.rs` + `q99.rs`: cache intake claims validate roots/formation before reuse (`validate_causal_cache_v1` L469, `record_tampering_and_noncanonical_replay_fail_closed`). `crates/zero-store/src/attempt_journal.rs`: `TornOrNoncanonicalRecord` failure (L158), immutable entries, digest-bound records. Tests: `tests/rust/zero-cert/golden_vectors.rs` `rejects_payload_range_digest_object_and_witness_tampering` (L38), `mutation_outcome_requires_an_independently_trusted_receipt` (L510). |
| **GAP** | Roots/formation/producer/scope/encoding checks exist. No tenant-identity binding (multi-tenant mode does not exist — see ZS-SEC-005), so the cross-tenant half of the acceptance is not testable/absent. |
| **CONFIDENCE** | high (single-tenant poisoning defenses are test-proven) |

## ZS-SEC-003 — Authority replay protection

| | |
|---|---|
| **STATUS** | **implemented** |
| **EVIDENCE** | `crates/zero-abi/src/raw_worker.rs` `ApprovalGrantRejection::{Expired, Replayed, BindingMismatch}` + `validate_approval_grant` (L283): single-use via `consumed_grants: &mut BTreeSet<String>`. `crates/zsx-core/src/session.rs`: `validate_session_approvals` (L1496) binds schema/root/generation/request_id/effect, expiry window <= 300s, dedup + `consumed_approval_ids` ledger (L209, capacity `MAX_SESSION_CONSUMED_APPROVALS` L91 in connector.rs), duplicate request-id rejection (L925), `StaleGeneration` on generation change (L910); `release_unadmitted` restores ledger on backpressure (L1278). `crates/zero-store/src/attempt_journal.rs`: recovery never redispatches; entries immutable after terminal. Tests: `tests/rust/zsx-core/unit/session.rs` `session_approval_contract_is_bounded_and_replay_safe` (L12, covers replay, wrong root, wrong effect, expiry, duplicate, capacity); `tests/rust/zero-store/unit/attempt_journal.rs` `attempt_recovery_never_redispatches` (L375). |
| **GAP** | Rollback/branch-switch replay: generation check covers epoch change and restart; branch-switch (new root) is covered by root binding; rollback replay relies on journal immutability. No dedicated test replays a captured grant after a rollback fixture. Minor. |
| **CONFIDENCE** | high (mechanism + tests are explicit and layered) |

## ZS-SEC-004 — Secret-safe rendering and logs

| | |
|---|---|
| **STATUS** | **missing** |
| **EVIDENCE** | Only `crates/zero-codemode/src/worker.rs` L391-392 `env_remove(SESSION_TOKEN_ENV)`/`env_remove(SESSION_SHUTDOWN_TOKEN_ENV)`. No redaction/secret-scrubbing code in zsx-core, zsx-node, zero-process, zero-gate (repo-wide grep for `redact`/`REDACT`/secret-rendering: zero hits). |
| **GAP** | No redaction policy for provider prompts, UI export, benchmark traces, or error strings; no secret fixtures in tests. Requirement entirely unimplemented. |
| **CONFIDENCE** | high (negative grep + only token-env removal exists) |

---

## ZS-OPS-001 — Crash recovery

| | |
|---|---|
| **STATUS** | **implemented** |
| **EVIDENCE** | `crates/zero-store/src/attempt_journal.rs`: `AttemptStateV1` lifecycle (L63), crash-boundary injection `crash_at: Option<AttemptBoundaryV1>` + `InjectedCrash` failure (L657-684), `recover_attempt_v1`/`recover_attempt_with_fault_v1` (L1114-1126) classification law (Succeeded/Failed/Indeterminate/SafeToRetry; Prepared -> SafeToRetry; DispatchCrossed -> evidence-classified, never redispatch), fsync+atomic rename via `fs_replace` (module doc L34-58). `crates/zero-store/src/session_wal.rs`: append/replay with torn-tail prefix retention + trailer mismatch fail-open (tests L9-L61). `crates/zsx-core/src/session.rs`: `reconcile_request` (L1044), `reconcile_all_attempts` (L1078); `crates/zsx-node/src/tasks.rs` `ReconcilePendingTask` (L256). `crates/zerostack-machine-permit/src/lib.rs`: dead/incomplete permit reclaim (`INCOMPLETE_PERMIT_GRACE` L48, `permit_status` L875). Tests: `tests/rust/zero-store/unit/attempt_journal.rs` `attempt_crash_boundaries_prepare_classify_safe_to_retry` (L413), `attempt_crash_boundaries_dispatch_classify` (L462), `attempt_crash_boundaries_terminal_entries_are_immutable` (L548), `attempt_recovery_outcomes_for_every_state` (L217); `tests/rust/zero-store/unit/durable_journal.rs` `journal_recovery_owner_death_is_typed_and_completes_safely` (L63), `journal_recovery_root_disagreement_is_never_guessed` (L92). |
| **GAP** | In-flight sandbox/lease reconciliation exists for journals and permits; no single end-to-end kill/restart-at-every-state integration fixture spanning session->journal->commit (component fixtures cover each state deterministically). Minor. |
| **CONFIDENCE** | high (crash boundaries cover every journal lifecycle state with deterministic fixtures) |

## ZS-OPS-002 — Concurrency control

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zsx-core/src/session.rs`: mutex-guarded admission with generation CAS + duplicate-request-id rejection (L905-940); `crates/zsx-core/src/connector.rs`: per-engine in-process serialization + bounded aggregate dispatch threads (`AGGREGATE_DISPATCH_THREADS=3` L94, `dispatch_permit_slots` L149); `crates/zero-store/src/cas.rs`: `StoreLock` publish/sweep (L301-315); `crates/zero-store/src/gc_lock.rs`; commit-race surfacing: `crates/zsx-node/src/envelope.rs` `CODE_COMMIT_RACE` (L58), `tasks.rs` L122; reachability snapshots `publish_reachability_snapshot`/`current_reachability_snapshot` (imported in connector.rs L28-30). Tests: `tests/rust/zsx-core/aggregate_world.rs` `one_call_atomically_commits_and_verifies_one_hundred_files` (L83). |
| **GAP** | No branch concept anywhere (grep: zero hits in zero-store/zsx-core); stale readers are explicit via StaleGeneration but no snapshot-isolation guarantee for concurrent readers; no race tests proving serializable authoritative roots beyond the single aggregate fixture. |
| **CONFIDENCE** | high (concurrency primitives present; branches/snapshot-isolation absent) |

## ZS-OPS-003 — Observability and trace export

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-abi/src/raw_worker.rs`: `WorkerTrace`, `TelemetryRequestV1` (L195: engine_stage_timeline, worker_token_accounting), `EngineStageTimelineV1` (L232), `WorkerTokenAccountingV1` (L205: raw/visible/recovery/billed/cached tokens). `crates/zsx-core/src/verdict.rs`: sealed `VerdictLoopReceiptV1` (L72) with bounded meter (token/byte budgets, failure stickiness). `crates/zero-gate/src/durable_publication.rs`: `NativeDurabilityReceiptV1` (L109). `crates/zero-store/src/attempt_journal.rs` + session_wal: resource/attempt ledgers. Tests: `tests/rust/zsx-core/verdict_loop.rs`, `verdict_loop_canonical.rs`. |
| **GAP** | No trace export pipeline (no otel/export code), no decision-boundary annotation artifact, no cache-event/invalidation-reason log surface, and benchmarks are catalog-only (`benchmarks/` README; no sealed manifests). "Every benchmark result reproducible from sealed manifests" is not implemented. |
| **CONFIDENCE** | high (telemetry structs + sealed verdict receipts exist; export/manifest layer absent) |

## ZS-OPS-004 — Schema and object migration

| | |
|---|---|
| **STATUS** | **missing** |
| **EVIDENCE** | Only version-gated compatibility: `TWO_PHASE_SCHEMA_VERSION` (two_phase.rs L33), `ATTEMPT_JOURNAL_SCHEMA_VERSION_V1` (attempt_journal.rs L60), `SchemaVersionMismatch` failure codes (attempt_journal.rs L152; semantic_cut.rs `SchemaVersionMismatch`), assembly manifest `require_version` (assembly.rs L214). `migrat` grep: single comment in `crates/zero-store/src/gc_lock.rs` L18. |
| **GAP** | No deterministic object-migration transformations, no old/new root pairs with validation receipts, no round-trip/golden migration fixtures. Fail-closed version rejection exists but migration itself does not. |
| **CONFIDENCE** | high (negative grep; only version checks) |

## ZS-OPS-005 — Distributed worker trust

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-abi/src/assembly.rs`: `WorkerIdentityV1` (L180: engine, artifact_digest, worker_protocol_digest, semantic_contract_digest, operation_registry_digest, capability_catalog_digest) bound in `AssemblyManifestV1.validate` (L203+). `crates/zero-abi/src/raw_worker.rs`: `validate_response_frame` (L638), `validate_approval_grant` before action (L283). `crates/zsx-core/src/adapter.rs`: in-process `DomainAdapter` contract — connector performs every worker-side validation (approval consumption, typed frame validation, result binding, ref reachability, telemetry validation; module doc L5-19). `crates/zsx-core/src/connector.rs` `take_approval`/grant forwarding (L807). Tests: `tests/rust/zsx-core/aggregate_world.rs`, `tests/rust/zsx-core/unit/connector.rs`. |
| **GAP** | No out-of-process malicious-worker fixture proving "malicious worker output cannot acquire cache or commit authority" (raw-worker process path is in zero-process/zero-codemode, but the trust verdicts are validated in-process at the connector; cache authority for workers is not exercised). Distributed/remote workers are not addressed — trust is enforced only for in-process adapters + local process pool. |
| **CONFIDENCE** | medium-high (identity+frame+grant validation solid; remote-worker threat model untested) |

## ZS-OPS-006 — Invisible steady-state budget

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/zero-process/src/resource.rs`: `DEFAULT_IDLE_TREE_RSS_BYTES = 96MB`, `DEFAULT_ACTIVE_TREE_RSS_BYTES = 256MB`, `DEFAULT_ACTIVE_CPU_SECONDS = 300` (L6-8), `ProcessResourcePolicy::share()` per-worker idle shares (L30-40), pre-exec `setrlimit` RLIMIT_AS/RLIMIT_CPU (L136-176). `crates/zerostack-machine-permit/src/lib.rs`: heartbeat liveness (1s interval) for held permits. `crates/zsx-node/src/session.rs`: `"idle"` session status string (L107). |
| **GAP** | Targets are ~96-256MB RSS and 300s CPU, not <=0.1% CPU / <=500MB idle; there is no idle-CPU measurement or enforcement, no scheduling disclosure for heavier indexing, and no release-gate test with measured evidence (benchmarks/ is catalog-only). |
| **CONFIDENCE** | high (budgets exist at process level; idle-CPU/500MB/release-gate requirement unimplemented) |

---

## ZS-SEC-005 — Cross-project cache isolation

| | |
|---|---|
| **STATUS** | **missing** |
| **EVIDENCE** | No `tenant`/project-authorization concept anywhere in crates/ (grep: zero hits). `crates/zero-store/src/cas.rs`: `SharedCas` is a content-addressed global store keyed by sha256 with no capability/handle scoping (L172+); `get_verified` (L587) reads by digest alone. Q99 binding has `protected_use_class_digest` (q99.rs L160) but no authorization check. |
| **GAP** | Physical dedup exists (CAS) but handles/reads/capabilities/receipts are not tenant-scoped; no cross-project guessed-root test exists. Both the acceptance and the mechanism are absent. |
| **CONFIDENCE** | high (negative grep; CAS is open by digest) |

## ZS-RELEASE-001 — Versioned cumulative claim authority

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | Spec/docs side: `racc/v6/lineage/AUTHORITY_AND_SUPERSESSION_RULES.md` (authority order, retained vs superseded/corrected classification, principal corrections A-H incl. Draft-4 rewrite supersession C and Q99 coordinate-specificity D), `racc/v6/lineage/CORRECTIONS_AND_SUPERSESSIONS.md`, `racc/v6/lineage/CLAIM_LEDGER_V6.md`, `FORMULA_INDEX_D1_D6.md`. Runtime side: `crates/zero-gate/src/q99.rs` `Q99LabelV1::{Q99State,Q99Input,Q99Total}` (L949), `Q99ClaimRecordV1` (L964: label + comparison_identity_digest + workload_digest + exact 99/100 threshold + attained), `generate_q99_metric_claim_v1` (L1215). Tests: `tests/rust/zero-gate/unit/q99.rs` `state_and_input_claims_have_labeled_denominators_and_exact_integer_thresholds` (L348), `total_claim_rejects_double_counting_and_mixed_native_coordinates` (L555). Conformance: `conformance/CONTRACT.md` (supersession statement), `tests/python/test_engine_topology.py`. |
| **GAP** | The supersession table ships as docs only; no runtime or release-checker code rejects claims citing superseded formulations (Draft-4 rewrite formula, one-token framing, ambiguous Q99%). Only pyc artifacts of a former `conformance/scripts/check_freshness.py` remain; no checker source in tree. Q99 claims are coordinate-labeled and scope-bound, which is the sound foundation, but "reject superseded-as-current" enforcement is absent. |
| **CONFIDENCE** | high (docs + labeled Q99 claims verified; runtime checker absent) |

---

## Cross-cutting findings

1. **Verifier identity binding is the strongest thread**: zero-cert trusted locks + assembly `VerifierIdentityV1` + semantic-cut verifier identity digest + DCR verifier route — version/scope substitution fails closed in tests (`golden_vectors.rs` L78). This covers the acceptance cores of VERIFY-001/002 even though a "registry" object does not exist.
2. **Authority chain is layered and linear**: grant validation (raw_worker) -> session admission/ledger (zsx-core) -> G0-G8 two-phase gate (zero-gate) -> attempt journal (zero-store). Replay protection (SEC-003) and crash recovery (OPS-001) are the two implemented requirements and they are mutually reinforcing (recovery never redispatches; grants are consumed before dispatch).
3. **Biggest gaps**: OS-level sandbox confinement (SEC-001), secret redaction (SEC-004), migration (OPS-004), tenant isolation (SEC-005), human acceptance gate (VERIFY-006), successor-state verification (VERIFY-003). These six are entirely absent — no partial symbols, no fixtures.
4. **Successor/root chaining** (`successor_root`, `predecessor_receipt_head`, deoptimization `successor_receipt_head_digest`) is the seed for VERIFY-003 and RELEASE-001's "future-safe successor publication" but is not yet a verifier.
5. **CSV status vs repo**: all 19 rows were "Not implemented / audit required" in the CSV; actual repo state: 2 implemented, 11 partial, 6 missing. The CSV statuses are stale for SEC-003/OPS-001 and understate the partial implementations.

## Confidence calibration

- **high**: negative greps (redact/tenant/sandbox-primitives/branch/migration) + positive test lists were verified directly.
- **medium-high/medium**: VERIFY-002 (no delta-substitution fixture), OPS-005 (no malicious-worker fixture).
- No claims of green were made for untested acceptance criteria; every "implemented" call rests on named tests.
