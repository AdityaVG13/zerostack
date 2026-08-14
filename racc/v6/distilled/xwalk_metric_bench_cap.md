# Crosswalk Audit: ZS-METRIC-001..011, ZS-BENCH-001..012, ZS-CAP-001..006

Audited 2026-08 against:
- ZeroStack repo (`/Users/aditya/AI/ZeroStack`): `crates/zero-gauge`, `crates/zero-ledger`, `crates/zero-gate`, `crates/zero-abi`, `crates/zero-codemode`, `benchmarks/`, `conformance/`, `tests/`
- TokenZero repo (`/Users/aditya/AI/TokenZero`): `crates/tokenzero-core`, `crates/tokenzero-pulse`

CSV source: `racc/v6/implementation/IMPLEMENTATION_BACKLOG_V6.csv` (all 29 rows read in full). Note: the CSV `status` column says "Not implemented / audit required" for every row; this audit distinguishes *mechanisms present in code* from *row fully satisfied*.

## Summary

| ID | Title | STATUS |
|---|---|---|
| ZS-METRIC-001 | Complete resource ledger | **partial** |
| ZS-METRIC-002 | Paired baseline manifest | **missing** |
| ZS-METRIC-003 | Separate cache metrics | **partial** |
| ZS-METRIC-004 | Multi-resource feasibility solver | **missing** |
| ZS-METRIC-005 | Index amortization | **missing** |
| ZS-METRIC-006 | Frontier Closure decomposition | **partial** |
| ZS-METRIC-007 | Certified lower-bound ledger | **missing** |
| ZS-METRIC-008 | Compression-dividend budget | **partial** (core implemented) |
| ZS-METRIC-009 | Capability asset lifetime ledger | **missing** |
| ZS-METRIC-010 | Certified unavoidable-work lower bound | **missing** |
| ZS-METRIC-011 | Reasoning allowance sovereignty audit | **partial** |
| ZS-BENCH-001 | Task manifest schema | **missing** |
| ZS-BENCH-002 | Decision-boundary annotation | **missing** |
| ZS-BENCH-003 | Prefix policy trial | **missing** |
| ZS-BENCH-004 | Causal invalidation trial | **partial** (mechanism only, no trial) |
| ZS-BENCH-005 | Provider-miss insulation trial | **missing** |
| ZS-BENCH-006 | Transactional fault matrix | **partial** (journal fault matrix exists) |
| ZS-BENCH-007 | Windowed Q99 and recovery | **missing** |
| ZS-BENCH-008 | Paired quality evaluation | **partial** (certificate schema only) |
| ZS-BENCH-009 | Release claim gate | **missing** |
| ZS-BENCH-010 | Adaptive decision-depth annotation | **missing** |
| ZS-BENCH-011 | Residency capacity/Q99 curve | **missing** |
| ZS-BENCH-012 | L1/L2/L3 forced-miss matrix | **missing** |
| ZS-CAP-001 | Episode capture | **missing** |
| ZS-CAP-002 | Scope and precondition proof | **missing** |
| ZS-CAP-003 | Shadow-mode promotion | **missing** |
| ZS-CAP-004 | Failure syndrome store | **missing** |
| ZS-CAP-005 | Revocation/revalidation | **missing** |
| ZS-CAP-006 | Capability lifetime and Q99 interaction | **missing** |

Tally: implemented 0 / partial 8 / conflicting 0 / missing 21. No row is fully implemented; nothing conflicts with a requirement (no code found that contradicts a requirement, but several mechanisms overlap a requirement's *input* without satisfying its acceptance test).

---

## ZS-METRIC-001 — Complete resource ledger (P0)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | ZeroStack `crates/zero-ledger/src/lib.rs`: `TokenLedger` (L295-415) -- raw/declared input, 6 `ChargeClass` counters, model_output_tokens, model_calls, retries, `FreshWorkVector`; `check_accounting_complete` (L378-395) rejects unclassified/double-counted input; `ResourceGauge` (L581) append-only, no decrement; `DominanceReceipt` (L952) + `ExactnessGates` (L904). `crates/zero-ledger/src/causal_work.rs`: 8 exactly-one `CausalWorkClassV1` classes (candidate, verification, comparison, baseline, fallback, restoration, prewarm, residue; L21-58), `ParentCounterObservationV1::Unmeasured` (L101), `DeclaredEstimateV1` (L113) cannot mint measured receipts; `CAUSAL_WORK_TAXONOMY_VERSION_V1=3`. TokenZero `crates/tokenzero-core/src/lib.rs` `Accounting` (L171-204) and `crates/tokenzero-pulse/src/lib.rs` `PulseEvent` (per-call: tool, mode, raw/visible/recovery tokens, cache_hit, retry_count, failure, exact_ref_count, latency_ms, source_hash, ref_ids) with JSONL source of truth + sqlite sidecar + sha256 meta. Worker accounting: `crates/zero-abi/src/raw_worker.rs` `WorkerTokenAccountingV1` (L210-230, Exact/ConservativeUpperBound/Estimate), enforced in `crates/zero-codemode/src/worker.rs` (L1514-1519: unsolicited accounting rejected, requested-but-missing fails). Tests: `tests/rust/zero-ledger/unit/causal_work.rs` (conservation, residue closure, estimate-alias rejection, unmeasured-not-zero), `tests/rust/zero-ledger/exact_checks.rs`. |
| GAP | No tool args/returns bytes, uncached-input counter, wire/disk bytes, CPU/GPU, storage, reasoning, or maintenance charges; no reconciliation-to-provider-bills function ("ledger reconciles to provider bills" untestable). Failed speculation folds into `Candidate` class; restoration and prewarm are covered by classes. TokenZero latency is the only non-token coordinate. |
| CONFIDENCE | high |

## ZS-METRIC-002 — Paired baseline manifest (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | `benchmarks/` is a catalog only (`benchmarks/benchmarks.md`, `benchmarks/README.md`); no manifests. `crates/zero-gate/src/quality.rs` `DistributionalClaimV1` (L665-700) binds `benchmark_digest`/`raw_baseline_identity_digest`/`pairing_method_digest` -- a consumer of a paired manifest, but no manifest schema, serializer, or paired-run tooling exists. |
| GAP | Entire requirement absent: no machine-readable manifest, no diff-over-treatment-only check, no randomization metadata handling. |
| CONFIDENCE | high (grepped manifest/baseline across both repos) |

## ZS-METRIC-003 — Separate cache metrics (P0)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | `crates/zero-gate/src/q99.rs`: `Q99LabelV1` (L949-954: Q99State/Q99Input/Q99Total), `Q99ClaimRecordV1` (L964) with labeled denominator and exact integer thresholds; `VerifiedCausalWorkReceiptV1` (L569) for L2-valid causal reuse; tests `tests/rust/zero-gate/unit/q99.rs`: `state_and_input_claims_have_labeled_denominators_and_exact_integer_thresholds` (L348), `provider_eligibility_is_never_reported_as_a_hit_or_semantic_proof` (L249), `total_claim_charges_preparation_and_complete_work_against_paired_raw_baseline` (L488), `total_claim_rejects_double_counting_and_mixed_native_coordinates` (L555). |
| GAP | Only the 3 Q99 labels are separated. No provider-prefix ratio, no L3 hot residency, no Boundary-Q99 (string absent repo-wide), no complete-work ratio metric, no quality-regression metric, no strict-rescue metric, no gap-closure metric; no dashboard/API at all. |
| CONFIDENCE | high |

## ZS-METRIC-004 — Multi-resource feasibility solver (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | Zero hits for "feasib" in `crates/`, `tests/`, `conformance/`. |
| GAP | No solver, no exact rational intersection, no blocker reporting. |
| CONFIDENCE | high |

## ZS-METRIC-005 — Index amortization (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | Only a digest field `setup_amortization_horizon_digest` inside the benchmark harness `tests/zero-testkit/src/zero_bench_r.rs` (L305, L326, L3147) -- no computation, no break-even horizon. |
| GAP | No cost allocation over reuse campaigns, no strict/Q99 break-even, no cold-build denominator enforcement. |
| CONFIDENCE | high |

## ZS-METRIC-006 — Frontier Closure decomposition (P1)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | `crates/zero-ledger/src/fresh_work.rs`: `FreshWorkComponent` (L27-62: fresh/replayed/recovery/overhead), `FreshWorkVector` with checked `component_sum` (L161), per-action `ActionFreshWork` (L253), session `SessionFreshWork` aggregate with eta ppm (L338); causal_work residue closure guarantees exactly-one classification. |
| GAP | Decomposition is fresh/replayed/recovery/overhead -- not the required normalized preparation / prepared-path / novelty+fallback terms, and no "largest limiting burden" report or optimized/baseline ratio closure. |
| CONFIDENCE | high |

## ZS-METRIC-007 — Certified lower-bound ledger (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No "gamma" hits in `crates/zero-gate` or `crates/zero-ledger`; no disjoint charging maps, no overlap checker. Related but non-satisfying: `Q99MetricReceiptClaimV1`/`VerifiedQ99MetricReceiptV1` (`crates/zero-gate/src/q99.rs` L1098-1215) mint claims only from verified receipts. |
| GAP | No lower-bound components, no double-count rejection across maps, no closure Gamma <= 1 check. |
| CONFIDENCE | high |

## ZS-METRIC-008 — Compression-dividend budget (P1)

| Field | Finding |
|---|---|
| STATUS | **partial** (core scheduler implemented) |
| EVIDENCE | `crates/zero-gate/src/reinvestment.rs`: `NativeResourceVectorV1` (L126) with `componentwise_le`/`checked_add`/`checked_sub`/`same_coordinates` (L199-256), `ReinvestmentActionClaimV1` (L259), `verify_reinvestment_action_v1` (L417); refusal paths at L1169 (committed <= baseline_budget), L1175 (<= expanded), L1277 (measured <= reserved). Tests `tests/rust/zero-gate/unit/reinvestment.rs`: `portfolio_reserves_fallback_and_computes_coordinatewise_slack` (L347), `extra_budget_is_labeled_and_budget_mutants_fail_closed` (L417), `higher_effort_without_ordered_theorem_is_not_laundered` (L480), `incomplete_or_overbound_measured_work_fails_closed` (L641). Strict-mode baseline selection exists in `tests/rust/zero-gate/unit/quality.rs` `distributional_evidence_is_valid_but_selects_baseline_in_strict_mode` (L266). |
| GAP | Savings provenance (verified compression receipts funding extra candidates) is not explicitly linked into reinvestment; "strict rescue separately certified" is implicit via quality strict mode, not a dedicated certificate. |
| CONFIDENCE | high |

## ZS-METRIC-009 — Capability asset lifetime ledger (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No capture/proof/reuse/saved-work/maintenance/invalidation/retirement tracking. Spec-only definition in `racc/v6/implementation/V6_CANONICAL_SYSTEM_SPEC.md` §15 ("Verified capability accumulation"). `crates/zero-abi/src/capability.rs` is an unrelated CAS layout capability contract. |
| GAP | Whole requirement absent. |
| CONFIDENCE | high |

## ZS-METRIC-010 — Certified unavoidable-work lower bound (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No lower-bound components, no closure report, no L>Z rejection. Nearest neighbors: causal_work `Baseline` class (a denominator class, not a lower bound) and `DistributionalClaimV1` scope digests. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-METRIC-011 — Reasoning allowance sovereignty audit (P0)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | `crates/zero-gate/src/deoptimization.rs`: `BaselineReasoningEntryV1` (L217), `BaselineSafepointClaimV1`/`EvidenceV1`/`CertificateRecordV1` (L295-574), `FallbackReserveV1` (L176), `RouteUsageV1` (L137), `DeoptimizationPlanV1` with `for_quality_fallback`/`for_fail_closed` (L669-825); tests `tests/rust/zero-gate/unit/deoptimization.rs`: `exact_restoration_mints_linear_baseline_invocation_and_g8_closure` (L460), `bare_fallback_transaction_cannot_enter_g8_without_resume_authority` (L614); reasoning contract with `NativeStatePolicyV1` in test fixture (L44). Spec §14 "Reasoning sovereignty" (V6_CANONICAL_SYSTEM_SPEC.md). |
| GAP | No captured "context escape" or "stopping policy" fields; no explicit rule that lowering treatment reasoning limit or disabling a native tool invalidates the no-degradation certificate (mechanism exists but the audit/certificate invalidation is not implemented). |
| CONFIDENCE | high |

---

## ZS-BENCH-001 — Task manifest schema (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | `benchmarks/` holds only `benchmarks.md` (aggregate-codemode, native-warm-read catalog entries; crate microbenchmarks under `tests/benches/<crate>/` for zero-cert/zero-gate/zero-ledger/zero-ref) and `README.md`. No task manifests, no schema validation, no root sealing. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-BENCH-002 — Decision-boundary annotation (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No annotation tooling, blinded rubric, inter-annotator agreement, or adjudication anywhere (no "annotat" hits in crates/tests). |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-BENCH-003 — Prefix policy trial (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No trial harness. Nearest mechanism: TokenZero `crates/tokenzero-core/src/provider_cache.rs` `ProviderCacheEligibility` (L43) with policy_id/reason -- eligibility check, not a prefix-policy trial. |
| GAP | No raw-retained vs retrospective-rewrite vs stable-capsule comparison, no LCP/token/cost/latency/quality/expansion report. |
| CONFIDENCE | high |

## ZS-BENCH-004 — Causal invalidation trial (P0)

| Field | Finding |
|---|---|
| STATUS | **partial** (mechanism complete; trial absent) |
| EVIDENCE | `crates/zero-gate/src/invalidation.rs`: `SupportCompletenessClassV1` (L332), `DerivationAuthorityV1` (L347), robust-snap intake, essential-edges cannot launder support completeness; unit tests `tests/rust/zero-gate/unit/invalidation.rs`: `exact_and_sound_support_mint_opaque_authority_and_bind_the_cache_line` (L409), `essential_edges_and_heuristics_never_launder_support_completeness` (L482), `record_tampering_and_noncanonical_replay_fail_closed` (L528); contract vectors `tests/zero-testkit/src/invalidation_contract.rs`: `invalidation_contract_stale_race_and_replay_vectors_fail_closed` (L181), `invalidation_contract_missing_edge_returns_index_behind` (L233). |
| GAP | No benchmark trial injecting leaf/central/semantic-no-op/toolchain/environment/branch changes with measured invalidated/recomputed mass; no early-cutoff mass recording (early-cutoff subsystem optional/absent). |
| CONFIDENCE | high |

## ZS-BENCH-005 — Provider-miss insulation trial (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No trial. Provider-eligibility semantics exist (`crates/zero-gate/src/q99.rs` `provider_eligibility_is_never_reported_as_a_hit_or_semantic_proof` test) but no forced TTL/eviction/routing/model-key miss experiment. |
| GAP | Entire trial absent. |
| CONFIDENCE | high |

## ZS-BENCH-006 — Transactional fault matrix (P0)

| Field | Finding |
|---|---|
| STATUS | **partial** (journal fault matrix exists; full matrix not) |
| EVIDENCE | `crates/zero-gate/src/transaction.rs`: `EffectClosureManifestV1` (L292), `ClosedEffectBoundaryV1` (L398), journal commit/abort/recovery (L466-587); unit tests `tests/rust/zero-gate/unit/transaction.rs`: `rollback_class_must_cover_writes_and_raw_fallback_never_speculates` (L330), `external_transactional_writes_remain_explicit_restoration_debt` (L395), `candidate_commit_requires_matching_zero_cert_acceptance` (L497), `committed_recovery_refuses_to_invent_missing_acceptance` (L521), `journal_abort_and_recovery_claim_only_declared_effect_closure` (L553); fault matrix `tests/zero-testkit/src/journal_fault_matrix.rs`: `journal_fault_matrix_exercises_every_frozen_boundary` (L350), `durable_journal_model_matches_the_runtime_contract` (L358). |
| GAP | Matrix covers crash/journal/stale-root/undeclared-effect/verifier-disagreement classes; cancellation, storage corruption, and CAS-race rows are not explicitly enumerated as a matrix, and no benchmark report output exists (testkit returns a report struct, `JournalFaultMatrixReportV1`, not a published benchmark). |
| CONFIDENCE | high |

## ZS-BENCH-007 — Windowed Q99 and recovery (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | `Q99ClaimRecordV1` has no window field; no sliding-window reuse measurement, no time/work-to-Q99-after-change, no impossibility-interval reporting. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-BENCH-008 — Paired quality evaluation (P1)

| Field | Finding |
|---|---|
| STATUS | **partial** (certificate schema only) |
| EVIDENCE | `crates/zero-gate/src/quality.rs`: `QualityPairV1` (L83), `ProtectedMetricV1` (L43), `ExactNeutralCertificateV1` (L219), `PointwiseDominanceCertificateV1` (L318), `DistributionalClaimV1` (L665: paired_tasks, candidate_wins, protected_losses, ties, mean_gain_ppm, lower_confidence_gain_ppm, confidence_ppm); tests `tests/rust/zero-gate/unit/quality.rs`: `pointwise_vector_admits_only_no_regression_and_exact_payload` (L149), `distributional_evidence_is_valid_but_selects_baseline_in_strict_mode` (L266). |
| GAP | No blinded evaluation pipeline, no subjective dimensions, no factual-support or test/build-result scoring; it is a claim/certificate ABI, not an evaluation run. |
| CONFIDENCE | high |

## ZS-BENCH-009 — Release claim gate (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | `conformance/ATTESTATION.md` explicitly states runtime reports are local-only, not release/CI attestations; `conformance/scripts/check_freshness.py` validates report indexes only. No automated gate for theorem status, citation resolution, unsupported Q99 substitution, checksums, or negative results. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-BENCH-010 — Adaptive decision-depth annotation (P0)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | Related runtime mechanism only: `crates/zero-gate/src/semantic_cut.rs` (hidden-segment classification). No annotation dataset, no IAA, no Zero Execute call-count comparison. |
| GAP | Entire benchmark absent. |
| CONFIDENCE | high |

## ZS-BENCH-011 — Residency capacity/Q99 curve (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No capacity model or eviction-plan comparison. Nearest mechanism: `CausalCacheAssessmentRecordV1`/`CausalCacheAdmissionV1` in `crates/zero-gate/src/q99.rs` (L349-468). |
| GAP | No LRU/LFU/size-aware/causal-weighted/oracle plans, no capacity/Q99 measurement, no optimizer/checker time separation. |
| CONFIDENCE | high |

## ZS-BENCH-012 — L1/L2/L3 forced-miss matrix (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No layer model exists ("L1/L2/L3" hits are only line-span refs). TokenZero `provider_cache.rs` distinguishes provider-cache eligibility only. |
| GAP | Entire matrix absent; no forced-miss experiment, no per-layer labels. |
| CONFIDENCE | high |

---

## ZS-CAP-001 — Episode capture (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No episode store in either repo; "episode" appears only in spec text (`racc/v6/implementation/V6_CANONICAL_SYSTEM_SPEC.md` §15) and docs. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-CAP-002 — Scope and precondition proof (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | `crates/zero-abi/src/capability.rs` is a shared-CAS layout capability (hash algorithm/layout version), not a verified capability with preconditions/postconditions/verifier/rollback/invalidation deps. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-CAP-003 — Shadow-mode promotion (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No shadow evaluation of capabilities. Quality envelope (`quality.rs`) gates protocol changes but is not shadow-mode capability promotion with misses/regressions/strict-rescues/complete-cost report. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-CAP-004 — Failure syndrome store (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | Zero hits for "syndrome" in crates/tests. |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

## ZS-CAP-005 — Revocation/revalidation (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | No capability revocation. `crates/zero-gate/src/invalidation.rs` invalidates *cache lines* on dependency change -- a different object (content cache vs capability asset), though conceptually adjacent. |
| GAP | Entire requirement absent (no expired/revoked asset costing, no revalidation, no execution-authority removal). |
| CONFIDENCE | high |

## ZS-CAP-006 — Capability lifetime and Q99 interaction (P1)

| Field | Finding |
|---|---|
| STATUS | **missing** |
| EVIDENCE | Nothing beyond ZS-METRIC-009/ZS-CAP-005 findings (all absent). |
| GAP | Entire requirement absent. |
| CONFIDENCE | high |

---

## Cross-cutting observations

- **Mechanism vs trial split (BENCH rows):** invalidation, transaction/journal, quality, provider-eligibility, and Q99 machinery are implemented as *verification contracts with unit tests*; the benchmark *trials* themselves (manifests, annotations, forced misses, windows, capacity curves) do not exist. `benchmarks/` is explicitly a catalog, and `tests/benches/` holds only criterion microbenches (zero-cert verify, zero-gate decide, zero-ledger charge, zero-ref parse).
- **Capability/episode subsystem is entirely spec-level** (V6_CANONICAL_SYSTEM_SPEC.md §15) with zero code; ZS-CAP-001..006, ZS-METRIC-009 all confirmed missing rather than stretched to adjacent machinery.
- **zero-gauge is not a metric crate.** `crates/zero-gauge` is an ordinal-allocation gauge (tokenizer-locked `Gauge::allocate`, fixture certification) -- resource *identity*, not accounting. The accounting home is `zero-ledger` + `zero-gate` + TokenZero pulse.
- **Q99 reporting exists** (`q99.rs`: Q99State/Q99Input/Q99Total labeled claims, preparation charge, complete-work charge against paired raw baseline) -- the strongest partial foundation for METRIC-001/003 and BENCH-004/005/008.
- **No conflicts found:** nothing in either repo contradicts a requirement; the gap is absence, not contradiction.
- CSV status column ("Not implemented / audit required") is a corpus default; this audit upgrades 8 rows to partial based on code+tests.
