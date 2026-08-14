# Crosswalk Audit: ZS-VIEW-001..010 (TokenZero / decision views)

- Auditor: scouting subagent (crosswalk auditor)
- Source of truth: `/Users/aditya/AI/ZeroStack/racc/v6/implementation/IMPLEMENTATION_BACKLOG_V6.csv` (rows 49-57, 115; full requirement + acceptance_test text read from CSV)
- Primary repo: `/Users/aditya/AI/TokenZero` (read `AGENTS.md` first; `CLAUDE.md` only points at AGENTS.md)
- Secondary repo: `/Users/aditya/AI/ZeroStack/crates/zero-ref` (capsules/refs)
- Method: targeted grep + file reads; exact file:line evidence below; no test runs performed (audit-only pass).
- Status values: implemented / partial / conflicting / missing. Confidence: high/medium/low.

---

## ZS-VIEW-001 — Semantic decision view

Requirement: "Construct a harness-independent rooted object containing exact relevant facts, protected uncertainty, candidate choices, evidence roots, and expansion authorities." (P0, depends ZS-GRAPH-004; acceptance: finite decision-family checker confirms merged states induce identical protected next decisions)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | `TokenZero/crates/tokenzero-core/src/decision_view.rs` — `DecisionView` (L508-632), `DecisionView::render` (L532), `DecisionViewSection` (L218-339) with section kinds `StableSystemToolContract, StableProjectCapsule, StableTaskFamilyCapsule, StableTypedEffectSchema, VolatileLocusEvidence, VolatileWorkingTreeDelta, VolatileUserTask, VolatileUncertaintyCoverage, VolatileRecoveryRoutes` (L61-77); `DecisionUncertaintyMarker` with `kind: Exact/SoundOverapproximation/PartialCoverage/Heuristic/Unknown` (L100-176); `DecisionViewIdentity` binds source_root + model_profile + tokenizer + tool_schema + renderer_contract digests (L344-382); view digest is a root over rendered bytes + prefix geometry + token map (L780-801); recovery refs validated against `zero_ref::ZeroRefV1` (L638-652). Harness-independent: provider-neutral ("Prefix byte identity is not a claim of provider eligibility", L3-7; no hit/eligibility flag in `StablePrefixGeometry`, L389). Tests: `tests/core/inline/decision_view__tests.rs` — `decision_view_is_deterministic_and_preserves_anchors_and_routes` (L97), `capsule_identity_and_recovery_ref_mismatches_fail_loudly` (L259). Also `TokenZero/crates/tokenzero-core/src/reasoning_state.rs` `RawDecisionViewRecoveryRefV1` (L629-646) binds identity+digest of a decision view into opaque reasoning-state recovery. Hub side: `ZeroStack/crates/zero-ref/src/lib.rs` provides the strict portable ref grammar (`ZeroRefV1`, `verify_and_select`, golden vectors `ZeroStack/tests/rust/zero-ref/fixtures/zeroref_v1_vectors.json`; unit tests `ZeroStack/tests/rust/zero-ref/unit/lib.rs`). |
| GAP | **No "candidate choices" field anywhere in the view model.** "Exact relevant facts" is caller-supplied payload (TokenZero validates identities, never selects facts); "protected uncertainty" is caller-supplied marker that TokenZero refuses to upgrade (L100-103 comment). Acceptance test (finite decision-family checker proving merged states induce identical protected next decisions) does not exist in TokenZero or zero-ref -- it is GraphZero/ZS-GRAPH-004 territory, and nothing in TokenZero consumes it. |
| CONFIDENCE | high (all claim-relevant symbols located and read) |

---

## ZS-VIEW-002 — Stable capsule normal form

Requirement: "Represent large artifacts as causal key + formation receipt + payload root + stable capsule + expansion handle at first model exposure." (P0, depends ZS-KERNEL-003, ZS-STORE-001; acceptance: later expansion appends a new rooted result and never rewrites the previously emitted capsule)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | `TokenZero/crates/tokenzero-core/src/model_artifacts.rs` — `ModelCapsule` (L406-544) = source_root_digest + model_profile_digest + tokenizer_identity_digest + sorted/deduped `evidence_refs` (ZeroRefV1-canonical, L450-464) + sorted/deduped `token_page_digests` + `stable_prefix` + `dynamic_tail` + token counts + domain-separated digest `model_capsule_digest` (L672-719); `TokenPage` (L331-405) is source-anchored (`ZeroRefV1` whole-blob anchor must hash-match, L361-368), bounded (MAX_TOKEN_PAGE_TOKENS 4096 / MAX_TOKEN_PAGE_BYTES 1MiB), exactly expandable (`TokenPage::expand`, L399-405). Expansion handle: engine `zero.token.expand`/`expandMany` surface, `TokenZero/crates/tokenzero-engine/src/expand_params.rs` (`ExpandParams`, L1-206) and `engine_expand.rs` (`EXPAND_RAW_MAX_BYTES`, typed cap, L12-28). Engine-level stable/volatile split at first exposure: `engine_misc.rs` `TokenZeroEngine::cache_pack` (L22+) stores stable prefix and volatile tail as separate content-addressed payloads; a later pack with changed sources stores NEW payloads and reports `invalidation_reason: sources_changed` -- the previously stored blobs are never rewritten (content-addressed store). Tests: `tests/core/inline/model_artifacts__tests.rs` `capsule_digest_is_canonical_and_provider_locked` (L147), `token_page_is_bounded_source_anchored_and_exactly_expandable` (L123); `tests/engine/inline/engine_misc__cache_pack_manifest_tests.rs` (`cache_pack_manifest_replaces_atomically_without_temp_residue`, L3). |
| GAP | **Causal key + formation receipt (ZS-KERNEL-003) are absent.** No `causal key` / `formation receipt` field or type exists in `model_artifacts.rs`, `decision_view.rs`, or zero-ref ("receipt" appears in TokenZero only as response/recovery receipts in `lib.rs`/`output_novelty.rs`, unrelated to capsule formation). `ModelCapsule` is defined and tested in core but **no engine production path constructs it** (grep of engine/mcp/codemode crates for `ModelCapsule::new|TokenPage` yields nothing). The acceptance property (expansion appends a new rooted result, never rewrites the previous capsule) holds de facto for cache_pack blobs but has no policy/type enforcing it for the capsule normal form. |
| CONFIDENCE | high |

---

## ZS-VIEW-003 — Exact expansion

Requirement: "Resolve every omitted fact through a live rooted handle or return Unknown; never fabricate a summary-only recovery path." (P0, depends ZS-VIEW-002; acceptance: expansion returns exact bytes/facts for original root; stale root is explicit)

| Field | Finding |
|---|---|
| STATUS | **implemented** (with a documented masking exception) |
| EVIDENCE | `TokenZero/crates/tokenzero-recovery/src/lib.rs` — `RecoveryStore::expand` with selector/fragments/anchors; stale handling: ordinal alias with wrong generation -> `miss!("stale-ref")` (L1881), file ref stale -> `miss!("stale-ref")` (L1982-1983), `RefResolve::Stale => Err("stale-ref")` (L3038), `dangling-ref` (L1887); blob refs verify `verify_and_select` hash identity (zero-ref `DigestMismatch` class). `engine_expand.rs` — `expand_with_reload_on_miss` (L205-240) reloads the cache once after a local miss, then typed miss via `annotate_expand_miss` (never a fabricated summary); `raw: true` returns exact bytes up to `EXPAND_RAW_MAX_BYTES` (256 KiB, env-overridable) and fails typed `expand_raw_cap_exceeded` beyond it (L12-28); non-raw expands return the recovered body **masked only for unambiguous credential shapes** (`mask_expansion_secrets`, L139-196; stored bytes never modified). Tests: `tests/engine/inline/engine_expand__tests.rs` — `raw_expand_over_cap_fails_typed_with_fragment_repair_hint` (L57), `expand_masks_unambiguous_secret_unless_raw_authorized` (L89), `expand_masks_pem_private_key_block` (L118), `masking_ignores_prose_and_short_lookalikes` (L182), `expand_since_rejects_non_expandable_since_ref` (L163). Recovery-side exactness: `TokenPage::expand` (model_artifacts.rs L399) returns exact source bytes. |
| GAP | No literal "Unknown" verdict type on the expand path (misses are typed `ToolResponse` errors with ref-not-found/stale reasons, which satisfies the "or return Unknown" disjunct functionally but is not a ZS-KERNEL-004-style trivalent verdict). Masking on non-raw expand means non-raw bytes are not byte-exact for credential-shaped content -- documented contract (yevj), so only a doc-level conflict with "exact bytes" wording. |
| CONFIDENCE | high |

---

## ZS-VIEW-004 — Canonical provider rendering

Requirement: "Render stable system/tool/schema blocks, task arguments, views, references, and results deterministically for each adapter/model contract." (P0, depends ZS-KERNEL-001, ZS-CONTRACT-003, ZS-ADAPTER-003; acceptance: repeated rendering under randomized process/map order is byte-identical)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | Deterministic framing with versioned contract: `decision_view.rs` `RENDERER_CONTRACT` (L20, `tokenzero.decision-view.renderer.v1; framing=section-kind+decimal-byte-length+lf+payload+lf; order=caller-preserved; stable=...; volatile=...`) and `decision_view_renderer_contract_digest` (L634); section framing `append_section` uses fixed `"section {kind} {len}\n"` headers (L711-729); all digests domain-separated length-prefixed SHA-256 (`digest()` helpers). Deterministic ordering in engine: `engine/collect.rs` `entries.sort()` (L36), `rows.sort()` (L256), `leftovers.sort()` (L281), `canonical_roots.sort_by` (L208); env map `BTreeMap` in `engine/render.rs` `inner_env` (L76-78). View stability classes fixed: `DecisionViewSectionKind::is_stable` (L79-91). Tests: `tests/core/inline/decision_view__tests.rs` — `decision_view_is_deterministic_and_preserves_anchors_and_routes` (L97), `renderer_preserves_caller_order_and_rejects_stable_after_volatile` (L201), `prefix_boundary_must_be_a_real_token_boundary` (L217), `renderer_contract_digest_is_stable_and_nonzero` (L307). |
| GAP | No randomized map/process-order perturbation test (acceptance) anywhere in the tree -- determinism is structural (BTreeMap/sorts) plus one deterministic test, not fuzz/permutation-proven. Per-adapter/model rendering is only bound through `ExactTokenizerIdentity` (zero-gauge `ProviderLock`); there is no adapter-specific rendering variant table (renderer is single-contract). Task-argument rendering: `ExpandParams::from_tool_args/from_codemode_args` parse arg objects but rendering of task arguments into the view is caller-side. |
| CONFIDENCE | high |

---

## ZS-VIEW-005 — Append-only model history policy

Requirement: "Never retrospectively replace provider-visible prior messages or tool results. Evidence refinement appends a new message/reference under a new root." (P0, depends ZS-VIEW-002, ZS-VIEW-004; acceptance: LCP of successive histories equals full previous history except excluded dynamic envelope fields)

| Field | Finding |
|---|---|
| STATUS | **partial** (no provider-message history policy exists; append-only discipline present in subordinate stores) |
| EVIDENCE | Append-only accounting: `engine/ledger.rs` "Queryable, append-only accounting for served TokenZero responses" (L1), JSONL append (`tokenzero.ledger.v2` records, `LedgerWriter`, L177-230); `engine/usage_telemetry.rs` append-only JSONL (L1-10, `UsageRecord` closed schema). Exposure ledger: `engine/exposure.rs` `SessionExposureLedger` records first exposure turn and `reexpansions` (L30-99); a second reference sends the short ref instead of re-inlining (L1-7) -- i.e., previously emitted material is never replaced, only referenced again. Content-addressed stores never rewrite old blobs: `engine_misc.rs` cache_pack (new payloads + `invalidation_reason`, L22-140), recovery store blob CAS. LCP measurement exists for accounting: `engine/cache_meter.rs` `common_prefix` (L553-556), `stable_prefix_tokens`/`prefix_stability_ratio` (L364-371, L485-513). |
| GAP | **No type, policy, or test implements "provider-visible prior messages are never replaced" at the message-history level.** TokenZero is tool-level; the provider message envelope/history is harness-side. No LCP-of-successive-histories acceptance test exists (LCP is only computed between request texts for cache metering). Dynamic-envelope-exclusion-from-cache-key is not modeled. |
| CONFIDENCE | high |

---

## ZS-VIEW-006 — Stable capsule admission policy

Requirement: "Estimate raw-vs-capsule horizon cost, expansion probability, handling cost, and complete resource vector before choosing indirection." (P1, depends ZS-METRIC-001; acceptance: replay data predicts which representation wins; policy can be disabled when expansion burden dominates)

| Field | Finding |
|---|---|
| STATUS | **partial** (heuristic threshold exists; no horizon/expansion-probability model) |
| EVIDENCE | Engine capsule omission with exact-ref recovery: `engine/engine_read.rs` `local_payload_policy` + `capsule_exact_ref_threshold_bytes` (L347-369, config L40/L86, env-configurable); `tokenzero_core::make_capsule_with_recovery_ref` (`core/lib.rs` L582-644) builds a visible capsule + `exact_ref`; `finalize_capsule_omission` with `lossy_spans`/`lossy_policy_id`. Resource vector telemetry: `engine/cache_meter.rs` `ProviderUsage` (L25-40), `CacheMeterNormalized` with `stable_prefix_tokens`, `prefix_stability_ratio`, `prefix_eligibility_rate` (L250-310); `zero.token.cache_pack` returns `cacheable_tokens`, `volatile_tokens`, `estimated_cached_token_savings`, `prefix_stability_ratio`, `invalidation_reason` (`engine_misc.rs` L125-140). Policy-disabled path: `raw: true` / `Mode::Exact` bypasses capsule omission (`make_capsule_with_recovery_ref` L595-600; `engine_read.rs` L347 `if raw`). Tests: `tests/engine/inline/engine_read__capsule_policy_tests.rs` (threshold 8 bytes forces capsule, L16). |
| GAP | No horizon cost estimate, expansion probability, handling cost, or complete resource vector (ZS-METRIC-001) is computed before choosing indirection -- admission is a fixed byte threshold (read) or a fixed source list (cache_pack). No replay-data prediction ("which representation wins") and no evaluation against replay data. No test for policy-disable-when-expansion-burden-dominates beyond raw/Exact mode bypass. |
| CONFIDENCE | high |

---

## ZS-VIEW-007 — Volatility-aware commutative prefix ordering

Requirement: "Inside semantically commutative blocks, order long/stable material before volatile material according to measured survival score; never reorder noncommutative messages." (P1, depends ZS-VIEW-004; acceptance: exhaustive small permutations confirm maximum expected prefix survival; semantic-order tests prevent illegal reordering)

| Field | Finding |
|---|---|
| STATUS | **partial** (stable-before-volatile is structurally enforced; no survival-score measurement or permutation proof) |
| EVIDENCE | Structural stable-first enforcement: `decision_view.rs` `StableSectionAfterVolatile` error (L27, L538-541) -- stable sections must precede volatile ones in any view; `DecisionViewSectionKind::is_stable` (L79-91). Survival measurement exists: `engine/cache_meter.rs` `common_prefix` (L553) and `prefix_stability_ratio` (L304-310, L485-513). Ordering at exposure: `cache_pack` emits stable prefix then volatile tail (`engine_misc.rs` L77-92) -- a stable-first ordering inside one "commutative block". Test: `renderer_preserves_caller_order_and_rejects_stable_after_volatile` (tests/core/inline/decision_view__tests.rs L201). |
| GAP | No "measured survival score" is assigned to material; ordering is a fixed two-class rule (stable classes before volatile classes), not score-based within classes, and the renderer explicitly preserves caller order (L528-529: "Render the supplied sections without selecting, sorting, or dropping any"). No exhaustive-permutation acceptance test and no semantic-order tests for noncommutative messages. |
| CONFIDENCE | high |

---

## ZS-VIEW-008 — Per-model token accounting

Requirement: "Count rendered tokens with the provider/model tokenizer when available and retain byte counts when not. Bind counting method/version in ledgers." (P1, depends ZS-CONTRACT-002; acceptance: golden prompt fixtures match provider usage within documented accounting boundaries)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | Model-aware counting: `core/tokens.rs` `TokenizerFamily`/`TokenizerMetadata` with disclosed average chars-per-token (CL100K 4.0, O200K 4.0, SentencePiece 3.5; `approximate: true` -- "No tokenizer vocabulary is linked today", L172-205); `count_tokens_for_model` (L262-266), `tokenizer_metadata` model-id rules (L224-239), env resolution `TOKENZERO_MODEL|OMP_MODEL|OPENAI_MODEL` (L242-251); lexical fallback `count_tokens_lexical` (L410); byte counts retained: `TokenMass.saved_bytes` (`engine/ledger.rs` L121-127), `ExposureRow.byte_len` (`engine/exposure.rs` L23). Exact provider-tokenizer path when available: `core/model_artifacts.rs` `ExactTokenizerAdapter` (`encode` + `token_bytes` round-trip, L215-230) and `ExactTokenMap::tokenize` byte-verified (L270-298). Method/version bound in ledgers: `engine/ledger.rs` `VersionIdentity { crate_version, git_describe }` (L190-193), `racc_charge` fragment (L160-163); `engine/racc_gauge.rs` `lexical_tokenizer_identity()` binds `"tokenzero-lexical-v1"` digest (L12-20) and `charge_from_accounting` (L23-62) classifies expand/recovery/reexpansion; `model_artifacts.rs` `ExactTokenizerIdentity::ledger_identity` maps to `zero_ledger::TokenizerIdentity` with revision digest (L178-184). Tests: `tests/core/inline/model_artifacts__tests.rs` `exact_identity_is_zero_gauge_locked_and_zero_ledger_compatible` (L64), `exact_map_roundtrips_and_requires_real_token_boundaries` (L102); `tests/core/inline/tokens__visible_budget_never_exceeds.rs` (budget invariant incl. documented marker-floor exception); `tests/core/inline/token_classes__token_class_tests.rs`. |
| GAP | No real provider tokenizer is linked (all registered families are disclosed estimates, `approximate: true`) -- the exact-adapter interface exists but production `count_tokens` uses estimates/lexical fallback. No golden prompt fixtures matching provider usage (acceptance test) exist in `tests/core/fixtures` (only `ack2-golden.json`, `one-token-atoms.json` -- neither is a token-accounting golden). Ledger binds crate version + tokenizer identity digest, but not a per-count method-version stamp on each `TokenMass`. |
| CONFIDENCE | high |

---

## ZS-VIEW-009 — Decision-view sufficiency check

Requirement: "For finite or verifier-covered domains, prove the complete compatibility set maps to one protected decision; otherwise return Unknown/expand." (P1, depends ZS-VIEW-001, ZS-KERNEL-004; acceptance: two compatible hidden states with distinct protected decisions always invalidate the view)

| Field | Finding |
|---|---|
| STATUS | **missing** (in TokenZero/zero-ref scope) |
| EVIDENCE | Negative capability only: `decision_view.rs` `DecisionUncertaintyMarker` + `DecisionUncertaintyKind` (L104-176) -- TokenZero "renders it but never upgrades it" (L98-99); Unknown is a first-class kind that cannot be silently upgraded. `reasoning_state.rs` binds view digest/identity into opaque recovery envelopes (L629-710) so a view can be invalidated by identity/digest mismatch (`DecisionViewIdentityMismatch`/`DecisionViewDigestMismatch`, L945-976). No sufficiency proof machinery, no compatibility-set enumeration, no verifier interface in either repo. `zero-ref` is ref grammar only. |
| GAP | The requirement and its acceptance test are GraphZero/ZS-GRAPH-004/005 territory ("complete compatibility set maps to one protected decision"); nothing in TokenZero or zero-ref implements or consumes such a check. The dependency chain (ZS-VIEW-001, ZS-KERNEL-004) is also unimplemented per CSV. If the parent expects this to live in TokenZero, that would conflict with the repo law ("Never import FSZero/GraphZero; composition is the hub's job") -- hub-side (zero-graph) is the correct home. |
| CONFIDENCE | high (absence established by targeted grep across both repos) |

---

## ZS-VIEW-010 — Decision-view completeness witness

Requirement: "Every compact view must identify the protected decisions it supports, exact evidence roots, omitted classes, expansion handles, completeness grade, and baseline escape." (P0, depends ZS-VIEW-003, ZS-GRAPH-005; acceptance: removing a needed evidence class causes Unknown or a failed certificate; exact expansion reproduces the bound object)

| Field | Finding |
|---|---|
| STATUS | **partial** |
| EVIDENCE | Present fields in `DecisionView`/`DecisionViewSection`/`ModelCapsule`: exact evidence roots (`evidence_refs` in capsule L407-419; `recovery_refs` in `DecisionUncertaintyMarker` L140-156; both ZeroRefV1-canonical), expansion handles (recovery refs are live `tz://` handles; `volatile_recovery_routes` section L277-284; engine expand surface `expand_params.rs`), omitted-classes-like declaration (`DecisionUncertaintyKind` taxonomy + `lossy_spans`/`lossy_policy_id` in engine capsules, `core/lib.rs` L582-644), failed-certificate behavior on evidence mutation (view digest covers every section payload; `capsule_identity_and_recovery_ref_mismatches_fail_loudly`, decision_view__tests.rs L259; reasoning-state `DecisionViewDigestMismatch`). Exact expansion reproduces the bound object: `TokenPage::expand` + recovery store digest-verified expand (`verify_and_select`). |
| GAP | Missing fields: **protected decisions it supports** (no decision identifier anywhere in the view), **completeness grade**, **baseline escape**. "Omitted classes" is only an epistemic kind label, not an enumeration of omitted evidence classes. No test for "removing a needed evidence class causes Unknown or a failed certificate" (closest: digest-mismatch fail-loud tests, which cover tampering, not class-removal sufficiency). Completeness grading is hub/GraphZero (ZS-GRAPH-005) per dependency chain. |
| CONFIDENCE | high |

---

## Cross-cutting findings

1. **Core view/capsule types are orphaned from production paths.** `DecisionView`, `ModelCapsule`, `TokenPage`, `ExactTokenMap` are fully implemented, digest-canonical, and well-tested in `tokenzero-core`, but no engine/MCP/codemode crate constructs them (verified by grep; only internal consumers are `reasoning_state.rs` envelopes and `output_novelty.rs` for `ExactTokenMap`). The live "capsule" in the engine is a different, simpler type (`make_capsule_with_recovery_ref` -> `Capsule` in core lib.rs).
2. **Anything requiring decision semantics is hub-side by design.** TokenZero law: "Never import FSZero/GraphZero. Depend only on hub contract crates." So ZS-VIEW-009's sufficiency proof and ZS-VIEW-010's completeness grade + protected-decision identity are expected in hub zero-graph (ZS-GRAPH-004/005, both "Not implemented / audit required" in CSV) -- TokenZero correctly refuses to upgrade Unknown (decision_view.rs L98-99), which is the only enforcement point that exists.
3. **zero-ref (hub) covers only the portable blob-ref grammar** (`ZeroRefV1`: `fz|gz|tz://blob/<sha256>` with `#B`/`#L` spans, strict lowercase, golden vectors at `ZeroStack/tests/rust/zero-ref/fixtures/zeroref_v1_vectors.json`, unit tests at `ZeroStack/tests/rust/zero-ref/unit/lib.rs`, error classes incl. `DigestMismatch`, `Stale`-like classes). No capsule, view, receipt, or history types in zero-ref -- it is the "expansion handle" vocabulary (evidence refs, recovery refs, source anchors all validate through it).
4. **Determinism is structural, not permutation-proven.** Repeated-rendering acceptance (ZS-VIEW-004) and permutation acceptance (ZS-VIEW-007) lack fuzz/permutation tests; determinism relies on BTreeMap/sort + fixed framing + domain-separated digests.
5. **Token accounting is estimate-grade in production.** Exact provider-tokenizer counting exists only behind the `ExactTokenizerAdapter` interface (test-only adapters); production `count_tokens` uses disclosed char-per-token averages or lexical fallback, and no golden fixtures validate against provider usage.
6. **CSV status column** for all ten rows reads "Not implemented / audit required"; this audit finds that label stale for ZS-VIEW-003 (implemented) and too blunt for 001/002/004/005/006/007/008/010 (partial).

## Suggested next steps for the parent

- Decide whether the "candidate choices" (ZS-VIEW-001) and "completeness grade / baseline escape" (ZS-VIEW-010) fields should be added to `DecisionViewSectionKind`/`DecisionView` in tokenzero-core, or deferred to hub zero-graph (recommended: defer; TokenZero should not own decision semantics).
- Gap candidates for TokenZero beads: real tokenizer link or golden-fixture accounting tests (ZS-VIEW-008); permutation/fuzz tests for rendering determinism (ZS-VIEW-004/007); a capsule-admission estimator with expansion probability (ZS-VIEW-006); a formal append-only history contract on the session exposure path (ZS-VIEW-005).
- Wire `ModelCapsule`/`DecisionView` into one engine surface (e.g., cache_pack or a new `zero.token.view`) or mark them dead code pending hub composition.
