# Crosswalk Audit: ZS-CACHE-001..015

Auditor: scouting subagent (crosswalk). Date: run at task time.
Sources: `racc/v6/implementation/IMPLEMENTATION_BACKLOG_V6.csv` (requirement+acceptance text), `Q99_CAUSAL_CACHE_IMPLEMENTATION_SPEC_V6.md` (Draft 6), plus code in:
- /Users/aditya/AI/ZeroStack (crates/zero-gate, zero-ledger, zero-abi, zero-store, zero-gauge, zsx-core, zsx-node)
- /Users/aditya/AI/GraphZero (graphzero-core, graphzero-engine)
- /Users/aditya/AI/TokenZero (tokenzero-core, tokenzero-engine, tokenzero-recovery)

## Landscape (what exists today)

| Capability | Where | Status |
|---|---|---|
| Causal cache keys (operator, canonical params, dep/env/toolchain roots, completeness witness, scope/anti-dep roots) | `ZeroStack/crates/zero-abi/src/cache_entry.rs` (`CacheKeyV1`, `CacheEntryV1`, `CompletenessWitnessV1`); contract `conformance/contracts/cache-entry-v1.md`; tests `tests/rust/zero-abi/unit/cache_entry.rs` | Implemented, shared contract |
| Same contract, parallel re-implementation (does not import zero-abi) | `GraphZero/crates/graphzero-engine/src/witness_cache.rs` (`CacheKey`, `CompletenessWitness`, `NoMatches` negative entries) | Implemented but standalone; NOT wired into graphzero-store query paths (only `pub mod` in engine lib.rs:20) |
| Invalidation cone (upward closure, sound overapprox, incremental-vs-full equivalence, missing-edge detection) | `GraphZero/crates/graphzero-core/src/invalidation.rs` (`DependencyGraph::upward_closure`, `certify_invalidation`, `RecomputeEngine::assert_incremental_equivalence`) + `dirty.rs` (`dirty_from_closure`); inline tests lines 387+ | Implemented + tested |
| Proof-carrying Q99 claims/certificates (Q99-State/Input/Total), 9 cache coordinates, validity enum, negative space | `ZeroStack/crates/zero-gate/src/q99.rs` (1678 ln; `CacheCoordinateV1`, `CacheValidityV1`, `CausalCacheBindingV1`, `CausalCacheComponentClaimV1`, `Q99MetricReceiptClaimV1`, `Q99ClaimRecordV1`, `generate_q99_metric_claim_v1`, `generate_q99_total_claim_v1`, `q99_contract_manifest_v1/v2`); tests `tests/rust/zero-gate/unit/q99.rs` | Implemented + tested, but zero-gate is depended on by no engine/harness (only zero-ledger); unwired |
| Cache-line-bound invalidation authority | `ZeroStack/crates/zero-gate/src/invalidation.rs` (`CausalCacheBindingV1` with `invalidation_certificate_digest` + `recovery_route_digest`, `BoundCausalCacheInvalidationV1::authorizes`, intake claims) | Implemented + tested |
| Fresh-work accounting vector (exact mass sums, eta_action) | `ZeroStack/crates/zero-ledger/src/fresh_work.rs` (`FreshWorkVector`, `SessionFreshWork`, checked sums, wire re-check) | Implemented + tested; unwired |
| Causal work receipts with counter windows + charges | `ZeroStack/crates/zero-ledger/src/causal_work.rs` (`CausalWorkReceiptV1`, `ParentCounterWindowV1`) | Implemented; `ParentCounterWindowV1` is a counter start/end measurement, NOT a sliding Q99 window |
| Verdict-loop cached-token budget meter | `ZeroStack/crates/zsx-core/src/verdict.rs` (`VerdictMeter`, `max_cached_tokens <= max_billed_tokens`, per-dispatch `cached_tokens`); wired via `connector.rs` `install_verdict_meter` | Implemented + used by harness; no layer distinction |
| Provider cache telemetry (usage parse, eligibility, telemetry statuses, SLO dashboard) | `TokenZero/crates/tokenzero-core/src/provider_cache.rs` (`ProviderCacheEligibility`, `ProviderCacheTelemetry` incl. `Unavailable/ReportedHit/ReportedMiss/ReportedUnknown/Expired`) + `TokenZero/crates/tokenzero-engine/src/cache_meter.rs` (`ProviderUsage`, `CacheObservation`, `CacheSessionReport`, `CacheSloConfig/Dashboard`, `cache_miss_attribution` via `cache-diagnosis` header) | Implemented + tested; TokenZero-only, not consumed by ZeroStack |
| On-disk action cache, tombstone-first eviction, shadow ledger, frecency | `TokenZero/crates/tokenzero-recovery/src/action_cache.rs` (`ActionCacheIndex`, `BlobEvictionPlan`), `cachezero.rs` (`CacheStatus`: ExactHit/CausalHit/SwrStale/CollapsedWait/ForcedMiss, shadow-only, graduation gate), `frecency.rs`, `prefix_stability.rs`; engine side `tokenzero-engine/src/cachezero.rs`, `action_cache_key.rs` | Implemented + tested |
| Integer compress-vs-cache crossover policy | `TokenZero/crates/tokenzero-engine/src/cache_crossover.rs` (`decide_cache_crossover`, `CacheCrossoverReceipt`, `CacheCrossoverReason`, compression-admission gate) | Implemented + exported, no call site in emission path found |
| Content-addressed physical store with dedup | `ZeroStack/crates/zero-store/src/cas.rs` (`SharedCas::put_outcome` create-vs-dedup, `get_verified`); tests `tests/rust/zero-store/unit/cas.rs` | Implemented + tested |
| Dispatch attempt recovery law (rematerialization of a failed attempt without replay) | `ZeroStack/crates/zero-store/src/attempt_journal.rs` (`recover_attempt_v1`/`recover_attempt_with_fault_v1`, `AttemptRecoveryReceiptV1`) | Implemented + tested |

**Biggest structural finding:** the entire Q99 certificate layer (zero-gate q99.rs, invalidation.rs) and ledger layer (zero-ledger) are NOT dependencies of zsx-core/zsx-node or of TokenZero/GraphZero. They are contract/verifier primitives with tests, but no runtime produces their receipts. TokenZero/GraphZero implement their own cache machinery with no link to the Q99 claim path. `zero-gauge` (ordinal refs) is unrelated to cache metrics and has no consumers.

---

## ZS-CACHE-001 - Three-layer cache accounting

- **STATUS: partial** (semantic skeleton only; no layer measurement)
- **EVIDENCE**: `ZeroStack/crates/zero-gate/src/q99.rs:44-160` - `CacheCoordinateV1` (9 coordinates incl. `ProviderCache`, `Rendering`, `ReasoningContinuation`) and `CacheValidityV1` (`Exact`, `ProviderEligible`, `ProviderReportedHit`, `Unknown`, `Invalid`...) with coordinate-validity restrictions; negative space `q99_contract_manifest` line ~1364: `"provider_hit_as_semantic_validity"`, `"prefix_reuse_as_reasoning_continuation"` forbidden. Test `tests/rust/zero-gate/unit/q99.rs:249` `provider_eligibility_is_never_reported_as_a_hit_or_semantic_proof`. TokenZero `cache_meter.rs:250` `CacheObservation` separates `stable_prefix_tokens` (local) from `provider_telemetry` (provider-reported).
- **GAP**: No L1/L2/L3 accounting exists anywhere in the three repos (grep for l1/l2/l3 tiers, residency, layer metrics: no hits). No L3 (physical residency) demand-weighted measurement; no forced-eviction experiment (acceptance test unimplementable today); the L1/L2/L3 taxonomy of the theorem is not the gate's coordinate taxonomy, and nothing measures the three layers separately at runtime. `VerdictMeter` (`zsx-core/src/verdict.rs`) tracks a single `cached_tokens` bucket.
- **CONFIDENCE: high** (exhaustive grep across all three repos)

## ZS-CACHE-002 - Provider cache telemetry

- **STATUS: partial**
- **EVIDENCE**: `TokenZero/crates/tokenzero-core/src/provider_cache.rs:35-208` (`ProviderCacheEligibility`: policy_id, prefix_geometry_digest, breakpoint_after_tokens, reason; `ProviderCacheTelemetry:208-318`: `Unavailable/ReportedHit{NonZeroU64}/ReportedMiss/ReportedUnknown/Expired`, `from_reported_cached_tokens` is presence-sensitive). `tokenzero-engine/src/cache_meter.rs:150` `parse_provider_usage` (cache_read_input_tokens + `cache_read_input_tokens_reported`, cache_creation_input_tokens, input/output), `:219` `provider_cache_telemetry` (diagnosis-driven Expired/ReportedUnknown with contradiction checks), `:250` `CacheObservation`, `:320` `CacheSessionReport` (provider_reported_hit_rate, provider_expired/unknown/unavailable counts), `CacheSloConfig/Dashboard` (target/regression hit rates, error + novelty budgets). Tests: `tests/` under tokenzero-engine (cachezero observe tests), provider_cache inline tests.
- **GAP**: No time-to-first-token capture; no cache key in the observation; no route/model per observation (model only via action-cache-key `model_id`); acceptance "missing fields are Unknown, not zero" holds in the data model but there is no end-to-end recording that reconciles with a provider usage response. TokenZero-only; ZeroStack harness never consumes these observations.
- **CONFIDENCE: high**

## ZS-CACHE-003 - Causal reuse telemetry

- **STATUS: partial**
- **EVIDENCE**: `ZeroStack/crates/zero-ledger/src/fresh_work.rs` - `FreshWorkVector` (fresh/replayed/recovery/overhead; `new()` derives total with checked add; wire deserializer re-checks sum; `eta_action_ppm`; `merge`) - "mass accounting sums exactly". Gate-side "cannot count Unknown/invalid as hits": `zero-gate/src/q99.rs:85-160` `CacheValidityV1::Unknown/Invalid` never treated as reuse; negative space `unknown_or_approximate_as_strict_reuse`; test `q99.rs:248` (unit) and `q99.rs:287` `cache_authority_rejects_missing_components_and_unmatched_evidence`.
- **GAP**: Accounting is token-level, not per-request demanded *object* sets: no demanded weighted set, no valid-reused/invalid/recomputed/early-cutoff sets, no reason codes per object. No per-object weights (see 010). `CausalWorkReceiptV1` charges are per work-unit classes but not per-object reuse sets.
- **CONFIDENCE: high**

## ZS-CACHE-004 - Windowed Q99 service metric

- **STATUS: missing**
- **EVIDENCE**: Nearest artifacts: `ZeroStack/crates/zero-ledger/src/causal_work.rs:82-99` `ParentCounterWindowV1` (start/end counter measurement window) - not a sliding window; `zero-gate/src/q99.rs` has no window concept; `Q99ClaimRecordV1` is windowless numerator/denominator. Contract manifests list no windows.
- **GAP**: No sliding declared windows, no lower-window reporting, no post-change recovery reporting, no "Q99 unavailable until threshold work resolved" state, no impossibility events. Acceptance test cannot run.
- **CONFIDENCE: high**

## ZS-CACHE-005 - Time/work to Q99

- **STATUS: missing**
- **EVIDENCE**: Closest: `zero-gate/src/q99.rs:1363` `q99_total_charges` includes `"restoration"` and `"deoptimization"` as charge classes in the complete-work side; nothing tracks invalid mass over time or computes remaining mass to restore 99%.
- **GAP**: No restoration threshold formula, no invalid-mass-resolved-over-time tracking, no synthetic-mutation test. Full requirement absent.
- **CONFIDENCE: high**

## ZS-CACHE-006 - Rewrite crossover estimator

- **STATUS: partial**
- **EVIDENCE**: `TokenZero/crates/tokenzero-engine/src/cache_crossover.rs` - `CacheCrossoverInput` (provider, policy_id, token_unit_id, content_class Stable/Churn, original_tokens, compressed_tokens, compression_admission_id, common_overhead_tokens H, cached_read_multiplier_ppm d, min_cacheable_tokens floor), `decide_cache_crossover` (integer-only: inline vs compressed vs cached totals), `CacheCrossoverReason` (`CachedStableCheaperOrEqual`, `CompressionStrictlyBeatsCache`, `BelowCacheableFloor`, `ChurnIsNotCacheable`, `CompressionNotAdmitted`), default keeps cache when compression unadmitted - matches "no historical rewrite in normal CCNF mode". Exported from `tokenzero-engine/src/lib.rs:75-77`.
- **GAP**: No suffix-size, compaction-cost, or remaining-reuse-horizon inputs; no replay experiment; no call site found in the emission path (policy library only). First-use write cost is implicit (original_tokens) rather than measured.
- **CONFIDENCE: high** (module read fully; grep for call sites)

## ZS-CACHE-007 - Cache frontier planner

- **STATUS: missing**
- **EVIDENCE**: No planner code in any repo. Nearest: `TokenZero/crates/tokenzero-recovery/src/frecency.rs` (scoring/coldest selection) and `cachezero.rs` (shadow classification) - neither proposes resident sets under capacity/latency/invalidation budgets. `zero-gate` verifies claims only.
- **GAP**: No DP optimum, no heuristic proposal + independent capacity/closure check. Entire requirement absent.
- **CONFIDENCE: high**

## ZS-CACHE-008 - Residency policy and prefetch

- **STATUS: partial**
- **EVIDENCE**: Separation of prediction from authority: `TokenZero/crates/tokenzero-recovery/src/cachezero.rs` ("would-have-hit decisions without serving"; `CacheStatus` incl. `SwrStale`; graduation gate `CACHEZERO_GRADUATION_PCT`; doc line 3 "The serve path stays off until graduation") and `action_cache.rs:355` `has_in_flight_serve` pin; `frecency.rs` (decay/score); `prefix_stability.rs` (CacheablePrefix). Consistency classes `ExactHit/Swr/MustBlockRevalidate` in `tokenzero-engine/src/action_cache_key.rs:10-32`.
- **GAP**: No prefetch of high-hazard/high-demand closures (grep "prefetch"/"hazard": no hits); no hot placement tier; prediction never *serves* but there is also no path where prediction affects latency/cost; shadow ledger is off by default. Acceptance's "invalid object never served as valid" is only structurally guaranteed in shadow mode, not in a real residency path.
- **CONFIDENCE: high**

## ZS-CACHE-009 - Q99 statistical certification

- **STATUS: missing**
- **EVIDENCE**: No confidence-interval / zero-failure bound code in any repo (grep "confidence"/"binomial"/"299"/"zero-failure": only `zero-gate/src/quality.rs:683-684` `confidence_ppm` for a different quality-gain metric). No sample-size theorem implementation.
- **GAP**: 299-trial zero-miss bound, cluster-aware effective sample, warm-trace caveat - all absent.
- **CONFIDENCE: high**

## ZS-CACHE-010 - Demand-weight ledger

- **STATUS: missing**
- **EVIDENCE**: `zero-gate/src/q99.rs:964` `Q99ClaimRecordV1` has `observed_numerator`/`denominator` strings, `task_count`, `label`, `threshold_relation` (AtLeast99Of100) and rejects zero denominators (`q99.rs:1143` validate, `ZeroDenominator` at 1324) - but no demanded-object closure, no per-object weights, no window, no tier, no validity-semantics ledger. No ledger rejection logic per the requirement.
- **GAP**: No per-object positive demand weights, no declared Q99 coordinate+window ledger, no "report rejected when demanded objects/weights/window/tier/validity absent or inconsistent" behavior. Denominators are scalar token/work totals only.
- **CONFIDENCE: high**

## ZS-CACHE-011 - Residency-plan proposal and verifier

- **STATUS: missing**
- **EVIDENCE**: No residency-plan type in any repo. `zero-gate` is a claim verifier for Q99 ratios, not for residency plans; `TokenZero` eviction (action_cache `prepare_blob_eviction`) and frecency select blobs but perform no capacity/retained-mass verification; no tier-specific Q99 certificate exists.
- **GAP**: No independent checker of capacity + retained valid demand mass; no optimizer/authority module separation. Entire requirement absent.
- **CONFIDENCE: high**

## ZS-CACHE-012 - Q99 eviction slack guard

- **STATUS: missing** (eviction substrate exists, guard absent)
- **EVIDENCE**: Adjacent machinery: `TokenZero/crates/tokenzero-recovery/src/action_cache.rs:185-233` `prepare_blob_eviction` (index-first tombstone, grace period, in-flight serve pin, `may_delete_blob` only after grace), `frecency.rs:56` `coldest`. Nothing computes `sigma = W_R - 0.99*W`, removed demand mass, post-eviction certificate, or compensation admission.
- **GAP**: No demand-mass/slack computation anywhere; adversarial just-below/just-above-slack acceptance test impossible. Eviction safety is retention-mechanics-based, not Q99-certificate-based.
- **CONFIDENCE: high**

## ZS-CACHE-013 - L2-valid/L3-cold recovery

- **STATUS: partial**
- **EVIDENCE**: Logical-vs-physical separation exists structurally in TokenZero ActionCache (index entry `verified`, `dep_closure_ref`, `fszero_bookmark` vs blob residency with tombstone/grace/delete; `ServedArtifact` pin) and in `ZeroStack/crates/zero-store/src/attempt_journal.rs:1114` `recover_attempt_v1` (deterministic recovery/rehydration of a failed dispatch without replay; `AttemptRecoveryReceiptV1`). GraphZero store daemon/session also recovers sessions (`graphzero-store/src/store/daemon.rs`).
- **GAP**: TokenZero tombstones *invalidate* entries on blob eviction - logical validity is not preserved across L3 loss (no L2-valid/L3-cold state). The acceptance experiment (forced local eviction preserves L2 validity, measured fetch/rematerialization path rather than rediscovery) is not implemented; attempt recovery is dispatch-level, not object-residency-level. No measured fetch/rematerialization metric.
- **CONFIDENCE: high**

## ZS-CACHE-014 - Provider-miss bounded amplification metric

- **STATUS: partial** (certificate machinery complete; no measurement pipeline)
- **EVIDENCE**: `ZeroStack/crates/zero-gate/src/q99.rs:1250-1370` `generate_q99_total_claim_v1` implements `100*(preparation + sum complete_task_work) <= sum raw_baseline_task_work` (i.e. `1-(C+L)/B >= 0.99` for the model-visible coordinate) with complete-work charges = `["preparation","candidate","validation","verification","comparison","guards","rejection","restoration","deoptimization","fallback","residue"]` - all backend work separately charged; denominator = raw baseline. Gate rejects scope/denominator mismatch: same `comparison_identity_digest`, `workload_digest`, `work_profile`, unique task digests, `DuplicateWorkUnit`, `ZeroDenominator` (q99.rs:1278-1342). Tests: `tests/rust/zero-gate/unit/q99.rs:488` `total_claim_charges_preparation_and_complete_work_against_paired_raw_baseline`, `:555` `total_claim_rejects_double_counting_and_mixed_native_coordinates`.
- **GAP**: No runtime produces these receipts (zero-gate unwired); no live B/C/L measurement; no provider-miss event recording. The ratio exists only as a verifiable claim shape.
- **CONFIDENCE: high**

## ZS-CACHE-015 - Cross-session and branch deduplication

- **STATUS: partial**
- **EVIDENCE**: Content/contract/dependency identity: `ZeroStack/crates/zero-abi/src/cache_entry.rs` `CacheKeyV1` (operator identity+version, canonical parameters, minimum dependency roots, environment roots, toolchain roots, completeness witness, scope roots; `canonical_key_json`/`key_hash_hex`; `CacheEntryV1::positive/negative`); GraphZero parallel `witness_cache.rs` (same wire shape, `CACHE_ENTRY_SCHEMA_V1`); TokenZero `tokenzero-engine/src/action_cache_key.rs:61` `action_cache_key` (op + canonicalized args with store-root-relative path normalization = branch/session neutral, model_id, consistency_class). Physical dedup: `ZeroStack/crates/zero-store/src/cas.rs:247` `put_outcome` (create vs dedup), tests `tests/rust/zero-store/unit/cas.rs:111`. Cross-session: TokenZero ActionCache is on-disk under a shared store root (`<store_root>/tokenzero/actions/`), world_id field exists.
- **GAP**: Tenancy authorization absent - no tenant scopes in any key, `world_id: None` hardcoded at the only write site (`tokenzero-engine/src/cachezero.rs:99`), no unauthorized-resolution test. GraphZero witness cache duplicates the contract instead of importing zero-abi (drift risk) and is not wired into query paths, so no logical cross-repo dedup exists today. Branch-neutrality holds by construction but is untested.
- **CONFIDENCE: high**

---

## Cross-cutting risks

1. **Unwired Q99 layer (blocker for all P0 metric requirements)**: zero-gate + zero-ledger are depended on by nothing in the runtime; zsx-core depends only on zero-abi/zero-store (+optional graphzero). All Q99 claims are test-only artifacts today.
2. **Two parallel cache-entry implementations** (zero-abi `cache_entry.rs` vs GraphZero `witness_cache.rs`) - wire-compatible but not shared code; no conformance test linking them.
3. **Theorem taxonomy mismatch**: spec's L1/L2/L3 layers do not map onto the implemented 9-coordinate model (`CacheCoordinateV1`) or onto TokenZero's provider-prefix/ActionCache/FSZero-CAS split; requirement 001/010 references windows/tiers with no counterpart types.
4. **Missing-field semantics**: TokenZero telemetry is presence-sensitive (Unknown, not zero) - good; but `CacheSessionReport` documents `hit_rate` as "legacy ratio; unavailable cached-token telemetry contributes zero", which conflicts with the Unknown-not-zero rule at report level.
5. **Cachezero graduation gate** (20% causal-hit mass) is a policy constant with no Q99/certificate link; `SwrStale` counts as would-have-hit without a staleness proof.
6. No TTFT timing, no route capture, no per-object demand weights, no eviction slack math anywhere - the four most frequently referenced acceptance mechanics (001/002/004/010/012) are unbuildable from current types.
