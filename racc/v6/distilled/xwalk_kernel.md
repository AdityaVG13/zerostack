# RACC V6 Crosswalk Audit -- ZS-KERNEL-001..008, ZS-CONTRACT-001..004, ZS-BASE-001..003

Auditor: crosswalk kernel subagent. Repo: `/Users/aditya/AI/ZeroStack` (commit on disk as inspected).
Requirement text read from `racc/v6/implementation/IMPLEMENTATION_BACKLOG_V6.csv` (all 15 rows; CSV `status` = "Not implemented / audit required" for all rows, which is a corpus default, not repo truth).

---

## ZS-KERNEL-001 -- Canonical serialization

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-abi/src/schema.rs:36` -- `pub fn canonical_json(value: &Value) -> String`: recursive deterministic encoding, object keys sorted, no whitespace; `canonical_schema_json` (line 31) with `normalize_schema`.
- `crates/zero-abi/src/digest.rs:30` -- `contract_digest_hex` / `contract_digest` = SHA-256 over `canonical_json`, the single engine-shared digest path.
- Tests: `tests/rust/zero-abi/unit/schema.rs:189` (`canonical_json_sorts_keys`), `:158` (canonical number semantics), `:55`/`:62` (set-like order normalization); `tests/rust/zero-abi/unit/digest.rs:5` (`digest_is_stable_across_key_order`), `:12` (`digest_changes_on_content_change`).
- Boundary-side normalization/rejection: `crates/zero-abi/src/cache_entry.rs:137` `CacheKeyV1` custom `Deserialize` (lines 152-171) rebuilds through `Self::build` which validates/normalizes roots and rejects unwitnessed roots (`RootNotWitnessed`); `crates/zerostack-machine-permit/src/lib.rs:135-149` canonicalizes scope paths before hashing (`try_scoped_permit_base_for`), tested in `tests/rust/zerostack-machine-permit/unit/lib__canonical_scope_tests.rs:16` (`canonical_scope_aliases_share_one_base`).

**GAP** There is one canonical JSON layer but no *single versioned canonical byte representation registry covering every object class* (receipts, events, views, deltas, authority objects) -- `canonical_json` covers manifests/schemas while `zbf.rs` defines a separate binary format and per-type receipts define their own `canonical_bytes()`. No explicit "reject noncanonical encodings at the authority boundary" rejector; acceptance tests for locale/process-order perturbations are absent (only key-order stability is tested).

---

## ZS-KERNEL-002 -- Content roots

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-ref/src/lib.rs:224` `content_hash_hex`, `:248` `object_identity_hex`, `:55-58` `HASH_ALGORITHM = "sha256"`, `:50-53` `ZEROREF_MAJOR/MINOR`, `:446` `ZeroRefV1::verify_and_select` (resolved bytes must hash to ref identity, `DigestMismatch`).
- `crates/zero-abi/src/digest.rs:17` `sha256` / `DigestV1` = `[u8; 32]` (zero-abi `assembly.rs`).
- Algorithm/version bound into digest *preimages* via versioned domain strings: `crates/zero-abi/src/cwir.rs:30` `CWIR_TASK_DOMAIN_V1` + `TaskBodyV1 { contract_version, model_version }` (line 232); `crates/zero-gate/src/two_phase.rs:40-45` `TWO_PHASE_CONTRACT_DOMAIN_V2..V5`; `crates/zero-gate/src/transaction.rs:31-38` `REQUEST_DOMAIN_V1` etc.
- Tests: `tests/rust/zero-ref/golden_vectors.rs` + `tests/rust/zero-ref/fixtures/zeroref_v1_vectors.json`; `tests/rust/zero-abi/unit/digest.rs:12`.

**GAP** `DigestV1` / `CacheRootV1` (`cache_entry.rs:21`) are bare 32-byte/hex values -- the algorithm tag is a global const, not structurally bound inside every root; version binding exists only in some contract preimages. No statement/test that display names/timestamps are never used as semantic identity (timestamps do appear as liveness fields, e.g. machine-permit `heartbeat_at`).

---

## ZS-KERNEL-003 -- Formation receipts

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-abi/src/cache_entry.rs:95-135` `CompletenessWitnessV1` (checked roots), `:104` `VerifierReceiptV1` (independent verifier binds `receipt_root`), `:137` `CacheKeyV1` binds operator identity + canonical parameters + minimum dependency roots + environment roots + toolchain roots (+ negative-entry scope roots).
- `crates/zero-ledger/src/lib.rs:851` `TaskAcceptanceReceipt` and `:711` `ArchiveAttestation`, `:780` `PolicyEvidence` -- `ExactnessGates` (lib.rs doc lines 33-37) can only be raised by presenting a verified evidence handle.
- Tests: `tests/rust/zero-abi/unit/cache_entry.rs:126` (`mutation_without_witness_update_is_rejected`), `:115` (`missing_completeness_witness_is_rejected`), `:64` (`canonical_hash_is_stable_for_semantically_equal_keys`).

**GAP** The cache-entry receipt binds dependency/env/toolchain roots + verifier witness, but there is no payload-formation receipt binding *constructor identity, contract root, execution record, payload root, and formation time/epoch*; the "relabel an unrelated payload with a valid causal key" acceptance test is not implemented at the payload level.

---

## ZS-KERNEL-004 -- Trivalent epistemic verdict

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE** (Unknown exists and fails closed, but no Safe/Unsafe/Unknown verdict type exists anywhere in `crates/` -- `grep -rn "Unsafe"` over `crates/` and `tests/` returns zero matches)
- `crates/zero-gate/src/recovery.rs:890` `RecoveryDecisionV1::Unknown(RecoveryUnknownDecisionV1)`; doc line 894: "`Unknown` always requires frozen raw-baseline fallback"; `:916-964` Unknown produced for unknown fibers/nonbeneficial recovery, never Complete.
- `crates/zero-gate/src/q99.rs:93` `CacheValidityV1::Unknown`; `:543` `Unknown => prohibited = true` (Unknown cannot grant cache authority).
- `crates/zero-abi/src/robust_snap.rs:61` `SnapLevel::Unknown`; `:235` `UnknownCannotPass`.
- `crates/zero-abi/src/cwir.rs:129` `CwirCoverageV1::Unknown`; `:157` `CwirEpistemicProductV1::validate` rejects exact soundness with Unknown coverage/determinism.

**GAP** No shared trivalent `Safe | Unsafe | Unknown` verdict type; `crates/zsx-core/src/verdict.rs:48` `VerdictDecision` is binary `Pass | Fail` only. The acceptance test (fault injection removing one required premise must yield Unknown or Unsafe, never Completed) is not implemented; per-domain Unknown values fail closed, which satisfies the spirit but not the letter.

---

## ZS-KERNEL-005 -- Typed authority separation

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-gate/src/two_phase.rs:1215` `ExecutionPermit` -- linear capability: private fields, not Clone, constructible only via `prepare()` (`:1253`) after guards G0-G7; `:1801` `BrokeredExecution`; `:2572` `ReadyToFinalize`; `:2584` `finalize() -> FinalReceipt` (`:2776`). `validate_permit_record` rejects schema mismatch as `ForgedPermit` (`:1296`).
- `crates/zero-cert/src/lib.rs:331` `VerifiedEvidence` with private `certificate` field and a `compile_fail` doctest (lib.rs lines 5-9) proving external construction is impossible; `:262` `EvidenceCertificate`.
- Tests: `tests/rust/zero-gate/unit/two_phase.rs:720` (`state_machine_prepare_execute_finalize_is_complete_and_linear`), `:806` (`state_machine_forged_permit_unbounded_worker_semantic_cut_and_image_fail`); `tests/rust/zero-cert/property.rs` tampering property tests.

**GAP** Executor/verifier authority is unforgeable inside zero-gate, but the requirement's role separation (planner, model, retriever, cache optimizer cannot construct authority objects) has no module-boundary audit or tests; no replay/expiry/scope-mismatch authority tests (machine-permit `crates/zerostack-machine-permit/src/lib.rs:371` is a filesystem lease, not scoped execution authority).

---

## ZS-KERNEL-006 -- Append-only authoritative event log

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-ledger/src/lib.rs:293` ("Cumulative, append-only token counters"), `:411` ("One append-only charge"), `:613` (`apply`); crate doc (lines 17-21): "`ResourceGauge` exposes no API to decrement or rewrite history; overflow is a typed error, never a wrap."
- `crates/zero-store/src/session_wal.rs:10` ("append-only records; compaction is a snapshot rewrite the caller owns"), `:206` `replay()`.
- Crash-replay tests: `tests/rust/zero-store/unit/attempt_journal.rs:413-472` (`attempt_crash_boundaries_prepare_classify_safe_to_retry`, `attempt_crash_boundaries_dispatch_classify`).

**GAP** Token charges and per-request attempt journals are append-only, but there is **no unified append-only authoritative event log with parent roots** covering state transitions, evidence observations, cache decisions, executions, verification, authority issuance, commits, rollbacks, and resource charges (no `parent_root`/`prev_root` linkage found anywhere in `crates/`). The session WAL's torn tail fails open (prefix kept, session_wal.rs:118), so missing/reordered events are not detectable via root chaining. The killed-process replay acceptance test is not implemented at the event-log level.

---

## ZS-KERNEL-007 -- Versioned semantic ABI

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- Version constants: `crates/zero-abi/src/assembly.rs:25` `ASSEMBLY_ABI_CONTRACT_VERSION: u16 = 1`; `crates/zero-abi/src/job.rs:15` `TOKEN_JOB_ABI_VERSION_V1`; `crates/zero-abi/src/cwir.rs` `CWIR_CONTRACT_VERSION_V1`/`CWIR_MODEL_VERSION_V1`; `crates/zero-ref/src/lib.rs:50-53` `ZEROREF_MAJOR/MINOR` (doc: "peers with a different major must refuse before payload work"); `crates/zero-ledger/src/lib.rs:46` `RECEIPT_SCHEMA_VERSION = 2`; `crates/zero-gate/src/two_phase.rs:34` `TWO_PHASE_SCHEMA_VERSION = 5`.
- Fail-closed version checks: `crates/zero-abi/src/reasoning.rs:74` `SchemaVersionMismatch`; `crates/zero-gate/src/two_phase.rs:1296` `validate_permit_record` rejects stale schema as `ForgedPermit`; `deny_unknown_fields` on all wire types.
- Golden fixtures: `tests/rust/zero-ref/golden_vectors.rs` + `fixtures/zeroref_v1_vectors.json`; `tests/rust/zero-cert/golden_vectors.rs`; `tests/rust/shared/schema_golden.rs`; `tests/rust/zero-ledger/unit/causal_work.rs:150` (`causal_classes_archived_v2_fixture_stays_readable_without_rewrite` -- v2 fixture decodes under v3 without rewrite).

**GAP** Versioning is per-type/per-schema (many independent constants), not *one rooted ABI version* spanning task contracts, semantic objects, receipts, events, and results; "migrate with a rooted receipt" machinery is absent (mismatch only fails closed). No golden fixtures across multiple *releases*.

---

## ZS-KERNEL-008 -- No partial authority

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-store/src/durable_journal.rs:327` `DurableJournalV2` single-transaction 2PC (crate doc: "single-transaction digest 2PC with 64 KiB record cap, fail-closed torn records"); `prepare_journal_v1`/`commit_journal_v1`/`abort_journal_v1`/`recover_journal_v1` used by `crates/zero-gate/src/transaction.rs:22-28`.
- `crates/zero-store/src/attempt_journal.rs:384` `AttemptEntryV1` with `AttemptBoundaryV1` (`:186`) crash points; tests `tests/rust/zero-store/unit/attempt_journal.rs:413-472` crash at every boundary -- result is either no entry, `Prepared` (`SafeToRetry`), or `DispatchCrossed` (`Indeterminate`), never a half-state.
- `crates/zero-gate/src/two_phase.rs:2584` `ReadyToFinalize::finalize` linear, single-construction final receipt.

**GAP** Atomicity is per-mutation-journal and per-execution-finalize; there is no project-level "exact verified successor root becomes current XOR unchanged" CAS on an authoritative root, and no crash-injection around the full verify/authorize/commit loop (the acceptance test's every-instruction-boundary coverage).

---

## ZS-CONTRACT-001 -- Task contract

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-abi/src/cwir.rs:192` `CwirTaskContractV1` binds `task_kind`, `specification_digest`, `required_snapshot`, with self-referential id over versioned body (`TaskBodyV1`, line 232, domain `CWIR_TASK_DOMAIN_V1`); embedded in `CausalWorkIrV1` (`:833`).

**GAP** Only task kind + opaque specification/snapshot digests are structured. Acceptance criteria, protected dimensions, Unknown/subjective dimensions, side-effect policy, environment fixtures, initial project root, model/harness/tool contracts, budget, deadline, and fallback policy are not bound as fields -- they collapse into `specification_digest`. No field-mutation-invalidates-certificate test exists for the task contract.

---

## ZS-CONTRACT-002 -- Model invocation contract

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-abi/src/reasoning.rs:41` `ReasoningContractV1` binds model/backend/tokenizer/decoder identity digests, `tool_schema_digest`, reasoning mode/effort, `max_output_tokens`, reserved reasoning/visible/recovery tokens, native state policy, `provider_extension`; `identity_digest()` rooted.
- Paired checker: `crates/zero-abi/src/reasoning.rs:425` `verify_strict_no_downshift_v1` -- rejects effort downshift, mode/effort change, identity mismatch, `allow_effort_downshift`; `StrictReasoningAdmissionV1` (`:282-330`) records only token *additions*.
- Tests: `tests/rust/zero-abi/unit/reasoning.rs:124` (`strict_rejects_every_numeric_downshift_and_effort_flag`), `:77` (`strict_identity_mode_effort_state_and_provider_changes_reclassify`), `:219` (`admission_and_extension_tampering_fail_closed`).

**GAP** No explicit fields for stopping policy, system prompt root, or sampling parameters (provider/model/version, tokenizer/counting method, reasoning allowance, tool permission set, routing fields are covered; `provider_extension` is an unstructured escape hatch).

---

## ZS-CONTRACT-003 -- Harness contract

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | medium |

**EVIDENCE**
- `crates/zsx-core/src/adapter.rs:86` `AdapterBinding` binds `engine`, `worker_revision`, `semantic_contract_version`, `semantic_contract_digest`, `operation_registry_digest`, `ref_scheme` with fail-fast `validate()` (`:123`); re-validated by the connector at registration.
- `crates/zsx-core/src/adapter.rs:6` documents zsx as embedding a native session without harness authority.

**GAP** No harness contract type binding tool serialization, message ordering, transcript policy, cancellation semantics, native tool set, or adapter renderer version; cross-harness renderer-difference comparison tests do not exist. (`AdapterBinding` covers adapter identity/version only.)

---

## ZS-CONTRACT-004 -- Protected scope

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-abi/src/cwir.rs:129` `CwirCoverageV1` (`Full`/`Partial`/`ObservedOnly`/`Unknown`/...) + `CwirEpistemicProductV1` (`:157`) validation: exact soundness requires complete coverage; exact soundness cannot have Unknown determinism.
- `crates/zero-gate/src/q99.rs:543` `CacheValidityV1::Unknown => prohibited`; `crates/zero-gate/src/recovery.rs:894` Unknown requires raw-baseline fallback.

**GAP** No protected-scope type enumerating tests/API/behavior/security/performance/file effects/user-visible output/successor-state obligations; coverage is a coarse enum, and no test proves an uncovered property is represented as Unknown and cannot be advertised as equivalent.

---

## ZS-BASE-001 -- Native path preservation

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zsx-core/src/session.rs:11` ("without ever calling an adapter. Every harness embeds this native session"); `:459` `ZsxBuilder` with per-engine adapters optional; `:838` `execute` / `:849` `execute_with_approvals` (approvals are a grant surface, not a bottleneck).
- Tests: `tests/rust/zsx/one_process.rs:92` (`in_process_dispatch_proves_one_process_and_no_worker_spawn`), `:341` (`zsx_exec_source_has_no_process_spawn_or_session_socket_path` -- source-level guard), `:194` (`approvals_reachability_and_bounded_dispatch_survive_in_process`).

**GAP** Native in-process path exists and is guarded, but the acceptance test -- a conformance task that disables ZeroStack mid-run and completes through the same native tool path -- is not implemented.

---

## ZS-BASE-002 -- Reasoning allowance preservation

| Field | Value |
|---|---|
| STATUS | **implemented** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-abi/src/reasoning.rs:425` `verify_strict_no_downshift_v1`: paired baseline/treatment check fails any run with smaller reasoning ceiling (effort/mode/identity/tool-schema/state-policy changes or any numeric downshift rejected); `StrictReasoningAdmissionV1` (`:282`) records only non-negative token additions.
- Tests: `tests/rust/zero-abi/unit/reasoning.rs:124` `strict_rejects_every_numeric_downshift_and_effort_flag`, `:176` `strict_permits_only_visible_numeric_reinvestment`, `:77` reclassification test.
- Used at the gate: `crates/zero-gate/src/two_phase.rs:32` imports `verify_strict_no_downshift_v1`; guards G0-G7 in `prepare()` (`:1253`).

**GAP** Minor: "tool authority" is covered via `tool_schema_digest` equality (set identity, not per-tool permission granularity); consumption measurement is separate (zero-ledger) as required.

---

## ZS-BASE-003 -- Fallback reserve

| Field | Value |
|---|---|
| STATUS | **partial** |
| CONFIDENCE | high |

**EVIDENCE**
- `crates/zero-gate/src/recovery.rs:818` `raw_baseline_expected_cost` (ExactRecoveryCostV1), `:858` `raw_baseline_required`, `:846` `RecoveryUnknownReasonV1::RawBaselineCheaperOrEqual`, doc `:894` "`Unknown` always requires frozen raw-baseline fallback".
- Tests: `tests/rust/zero-gate/unit/recovery.rs:336` (`unknown_fiber_and_nonbeneficial_query_require_raw_baseline`), `:407` (strict canonical bytes).
- `crates/zsx-core/src/verdict.rs:165` `reserve_dispatch` -- metered budget reservation before dispatch (`VerdictMeter`, `:95`).

**GAP** The raw-baseline cost is *accounted* and Unknown refuses speculation (raw-baseline route chosen), but there is no time/token/compute/external-effect reservation pool with "injected late failure still permits baseline completion, or speculation refused before it begins" acceptance test; `reserve_dispatch` is a per-dispatch token reservation, not a fallback reserve.

---

## Summary table

| ID | STATUS | Confidence | Primary evidence |
|---|---|---|---|
| ZS-KERNEL-001 | partial | high | zero-abi `schema.rs:36` `canonical_json`, `digest.rs:30`; tests `zero-abi/unit/{schema,digest}.rs` |
| ZS-KERNEL-002 | partial | high | zero-ref `content_hash_hex` `:224`, `HASH_ALGORITHM` `:55`; domain-versioned preimages |
| ZS-KERNEL-003 | partial | high | zero-abi `cache_entry.rs` `CacheKeyV1:137`/`VerifierReceiptV1:104`; zero-ledger `TaskAcceptanceReceipt:851` |
| ZS-KERNEL-004 | partial | high | no Safe/Unsafe/Unknown type; `RecoveryDecisionV1::Unknown` `recovery.rs:890`, `CacheValidityV1::Unknown` `q99.rs:93` |
| ZS-KERNEL-005 | partial | high | two_phase `ExecutionPermit:1215`/`prepare:1253`; zero-cert `VerifiedEvidence:331` + compile_fail |
| ZS-KERNEL-006 | partial | high | zero-ledger append-only counters `lib.rs:293`; zero-store `session_wal.rs:10`; no parent-root event log |
| ZS-KERNEL-007 | partial | high | version consts (`assembly.rs:25`, `job.rs:15`, zero-ref `ZEROREF_MAJOR`); golden vectors; fail-closed checks |
| ZS-KERNEL-008 | partial | high | zero-store `DurableJournalV2:327`, `attempt_journal.rs:384` + crash tests `:413-472`; `ReadyToFinalize::finalize:2584` |
| ZS-CONTRACT-001 | partial | high | cwir `CwirTaskContractV1:192` (kind + opaque spec digest only) |
| ZS-CONTRACT-002 | partial | high | reasoning `ReasoningContractV1:41` + `verify_strict_no_downshift_v1:425`; no stop/sampling/system-prompt fields |
| ZS-CONTRACT-003 | partial | medium | zsx-core `AdapterBinding:86`; no harness contract type |
| ZS-CONTRACT-004 | partial | high | cwir `CwirCoverageV1:129`/`CwirEpistemicProductV1:157`; no protected-scope type |
| ZS-BASE-001 | partial | high | zsx-core `session.rs:11`/`:838`; `tests/rust/zsx/one_process.rs:92,341` |
| ZS-BASE-002 | **implemented** | high | `verify_strict_no_downshift_v1` `reasoning.rs:425` + `reasoning.rs` tests `:124,:176` |
| ZS-BASE-003 | partial | high | `recovery.rs:818/858/894` raw-baseline cost; verdict.rs `reserve_dispatch:165` |

## Notes for the implementer
- Zero "Unsafe" matches repo-wide: ZS-KERNEL-004 needs a real trivalent verdict type or an explicit mapping decision.
- No `parent_root`/`prev_root` linkage anywhere: ZS-KERNEL-006's parent-rooted event log and ZS-KERNEL-008's successor-root CAS are the two largest greenfield gaps.
- ZS-BASE-002 is the only row with a direct, tested match (`verify_strict_no_downshift_v1`).
