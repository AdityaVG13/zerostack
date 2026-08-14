# Crosswalk: RACC V6 ZS-GRAPH-001..010 vs GraphZero repo

Repo audited: `/Users/aditya/AI/GraphZero` (rev `35441f0`, main). Contract source: `/Users/aditya/AI/ZeroStack/racc/v6/implementation/IMPLEMENTATION_BACKLOG_V6.csv` (all 10 rows, full requirement + acceptance_test text read).
Repo law read first: `AGENTS.md` (rev-aware; "Incremental vs full must not diverge on protected claims"; "Sound overapprox invalidation may invalidate too much, never too little").

## Naming note (applies to GRAPH-003, and everywhere grades appear)

GraphZero does **not** use the V6 grade names Proved/BoundedComplete/Observed/Unknown. Its equivalent lattice terms are:
`CoverageClass {Complete, SoundOverapproximation, ObservedOnly, Partial, Unknown}` (graphzero-core), `TruthClass` (8 classes incl. first-class `Unknown`), `ClosureClass {Exact, SoundOverapproximation, Heuristic, Incomplete}`, `FiberClass {Exact, SoundOverapproximation, Underapproximation, Unknown}`, `QueryResult {Present, Absent, Unknown}`. Mapping used below: Complete~Proved, SoundOverapproximation~BoundedComplete, ObservedOnly~Observed.

---

## ZS-GRAPH-001 -- Incremental repository index

| Field | Finding |
|---|---|
| STATUS | **implemented** (partial on two sub-bullets: generated artifacts, ownership edges) |
| Requirement | Language-aware files, syntax trees, symbols, references, imports, calls, tests, configuration, build, generated artifacts, ownership edges under exact snapshot roots |
| Evidence | `crates/graphzero-store/src/store/indexer.rs`: `index_repo()` (L4190), `collect()` (L3192), `collect_changed_paths()` (L3072), worktree fingerprint `collect_worktree_fingerprints()` (L4488) + `refresh_changed_worktree_fingerprints()` (L4570), incremental threshold logic (L4227-4231), `try_fast_warm_index`, delta-log append path (`compaction.rs` L478). Language-aware extraction: `crates/graphzero-extract/src/engine.rs` `extract_tier_a`/`extract_batch` (tree-sitter, Rust/TS/Python; Contains/Calls/Imports/Implements edges with confidence bands); SCIP ingest `crates/graphzero-scip/src/ingest.rs`; semantic tier `crates/graphzero-semantic/src/index.rs`; config: Cargo.toml workspace parsing (`indexer.rs` L1717, L1870, L2596); tests: blast `CoveringTest` mapping (`graphzero-engine/src/blast.rs` L57, L533-534). Snapshot roots: `SourceAnchor {source_root, producer, configuration}` (`graphzero-core/src/graph.rs` L70-77), snap-to-edit HIT refs (`graphzero-store/src/store/query/snap.rs`). |
| Tests | `index_repo_uses_incremental_collect_for_small_changed_set` (indexer.rs L5546, asserts 0 extra extractions); `one_file_watch_moves_prior_data_and_preserves_full_collect_parity` (L5684) with `assert_full_parity`: `load_index_content_signature == index_content_signature(full collect)`; `tests/store/incremental_watch.rs`: `changed_path_patch_matches_full_reindex_without_reparsing_unchanged_files`, `duplicate_burst_is_coalesced_and_matches_full_reindex_after_storm` |
| GAP | (a) Acceptance test names "known benchmark repos" -- parity tests use synthetic fixtures only, no benchmark corpus. (b) "generated artifacts" and "ownership" edges: no explicit population; `Relation::Tests/BuildDepends/SchemaDepends/EffectMayTouch` declared in `graphzero-core/src/graph.rs` (L56-67) but unused by the store indexer. (c) no syntax-tree persistence (tree-sitter facts are projected to symbols/edges; no CST store) |
| CONFIDENCE | High -- incremental == full parity is tested at the content-signature level |

## ZS-GRAPH-002 -- Causal dependency graph

| Field | Finding |
|---|---|
| STATUS | **partial** (files/env/tool-version/verifier-input roots yes; network fixtures, clocks/randomness policy not represented) |
| Requirement | Deterministic constructors + every declared dependency: files, environment, tool versions, network fixtures, clocks/randomness policy, generated files, verifier inputs |
| Evidence | `crates/graphzero-core/src/invalidation.rs`: `DependencyGraph` (L48), `add_dependency` (L113), `upward_closure` (L126), `certify_invalidation` (L142), `InfluenceClass {ExactSupport, SoundOverapproximation, Heuristic}` (L23), `DependencyClosureRecord` (L85), `record_dependency_closure` (L358), `dirty_from_closure` (journal-delta x closure, L374). Deterministic constructors: `ProducerFn`/`RecomputeEngine` (L194-227). `crates/graphzero-engine/src/witness_cache.rs`: `CacheKey` with `minimum_dependency_roots`, `environment_roots`, `toolchain_roots`, `scope_roots` (anti-dependencies) + `CompletenessWitness` (L180-215) -- file/env/toolchain as causal roots. Verifier inputs: `crates/graphzero-core/src/effect_map.rs` `VerifierObligation`/`VerifierObligationMap` (L77-101). Declared-but-unused relation kinds incl. BuildDepends/SchemaDepends (graph.rs L56-67). |
| Tests | In-crate: `missing_edge_causes_under_invalidation_detection` (hidden dependency change => divergence, never silent reuse), `unrelated_change_does_not_invalidate_sibling`, `dirty_from_closure_intersects_journal` |
| GAP | No representation of network fixtures, clocks/randomness policy, or generated-file causal roots. DependencyGraph nodes are bare content hashes with no declared-dependency kinds. Witness cache covers env/toolchain roots but only for cached query ops, not the full build/test graph |
| CONFIDENCE | High on files/env/toolchain; the missing declared-dependency categories are confirmed absent by grep |

## ZS-GRAPH-003 -- Dependency assurance lattice

| Field | Finding |
|---|---|
| STATUS | **partial** (semantics largely present under different grade names; no upgrade/revocation mechanism found) |
| Requirement | Grade graph regions Proved/BoundedComplete/Observed/Unknown; only first two may authorize omission/exact reuse in explicit scope; upgrades require evidence and are revocable |
| Evidence | `graphzero-core/src/graph.rs` `CoverageClass` (L25-53): `permits_absence_certificate()` = Complete only (L35); `certify_absence` refuses `CoverageNotComplete` (L214-241); `NegativeKnowledgeCertificate` (L100-109). `truth.rs` `TruthClass` with first-class `Unknown`, `may_upgrade_to_exact()` always false (no proximity upgrades). `decision.rs` `ClosureClass`/`is_decision_complete` (Exact|SoundOverapproximation + no gaps). `world_fiber.rs` `FiberClass` strict-admissibility. `graphzero-coverage/src/query.rs` `QueryResult {Present, Absent, Unknown}` every variant carries `CoverageCertificate`; Absent only when `freshness_verified && full_tier_indexed` |
| Tests | In-crate: `absence_refused_without_complete_coverage`, `unknown_coverage_never_proves_absent`, `heuristic_never_upgrades_to_exact`; coverage crate: `test_absent_certificate`, `build_degrades_invalid_present_to_unknown` |
| GAP | (a) Grade names differ from V6 (see naming note) -- semantic mapping required, so cross-repo conformance is a judgment call. (b) GraphZero is *stricter*: `SoundOverapproximation` does NOT permit absence certification (V6 allows BoundedComplete inside explicit scope) -- divergence in authorization boundary. (c) No API to upgrade an assurance grade with evidence, nor to revoke a previously granted grade (grades are fixed at construction from extractor/coverage) |
| CONFIDENCE | High on implemented semantics; naming/authorization divergence is real and confirmed |

## ZS-GRAPH-004 -- Task-relative causal lens

| Field | Finding |
|---|---|
| STATUS | **partial** (all ingredients exist, no composed lens; no interface list; no counterexample loop) |
| Requirement | Given task contract + candidate plan: demanded evidence/write closure, assurance grade, source roots, tests, interfaces, unresolved ambiguities |
| Evidence | `graphzero-core/src/atlas.rs`: `TaskFingerprint` -> `AddressAtlas::resolve` -> `SnapLevel {S0,S1,S2,Unknown}` + `LocusRank` (truth + premises; top rank is certificate only at S0). `graphzero-core/src/decision.rs`: `DecisionClosure::assemble(task, evidence, gaps)` with `EvidenceKind {Definition, Type, BuildProfile, GeneratedArtifact, TestOrVerifier, RuntimeEdge, UnresolvedGap}` and `gaps` = unresolved ambiguities. `graphzero-engine/src/blast.rs`: `impact_before_edit` / `SpeculativeBlastRequest` / `PlannedEdit` (tests/engine/impact_before_edit.rs). `graphzero-engine/src/rewrite_closure.rs`: `rewrite_closure(snapshot, root_symbol, PropagationPolicy)` -> `RewriteClosure {sites, unresolved_sites, accounting}` (output-side implied edit closure, HIT refs). Source roots: `SourceAnchor`. Tests: `tests/engine/rewrite_closure_sites.rs` (independent identifier scan cross-check) |
| GAP | No single entry point taking (task contract, candidate plan) and returning the demanded (evidence closure, write closure, grade, source roots, tests, interfaces, ambiguities). Evidence kinds + gaps (decision.rs) come closest; write closure is blast/rewrite_closure. "Omitted edge becomes a counterexample" acceptance: tests verify against independent scans but there is no retained-counterexample loop (see GRAPH-008) |
| CONFIDENCE | Medium-high -- composition exists but is spread across atlas/decision/blast/rewrite_closure with no task-contract type |

## ZS-GRAPH-005 -- Executable closure checker

| Field | Finding |
|---|---|
| STATUS | **implemented** (core-level; matches acceptance test closely) |
| Requirement | Given known values, cached objects, constructors, demanded outputs: deterministically verify all outputs lie in the derivable closure |
| Evidence | `graphzero-core/src/invalidation.rs` `RecomputeEngine`: `full_recompute` (L227, data-availability fixed point -- computes exactly the derivable closure from constructors `ProducerFn`), `incremental_recompute` (L278), `assert_incremental_equivalence` (L316, fail-closed on `EquivalenceDivergence`). Witness side: `CompletenessWitness::recomputes()` (`witness_cache.rs` L168-177) proves a cached closure's checked-roots set recomputes to its proof root |
| Tests | In-crate: `incremental_equals_full_on_protected_dag` (DAG fixture: incremental == exhaustive full evaluation), `missing_edge_causes_under_invalidation_detection` (rejects missing constructor/input edge), `heuristic_influence_cannot_claim_protected_equivalence` |
| GAP | Checker validates over *all* producers, not a demanded-output subset; no standalone "is this output derivable?" predicate exposed. Missing-input rejection only surfaces via equivalence divergence, not a direct closure-membership error |
| CONFIDENCE | High -- the finite-DAG agreement requirement is literally what the in-crate tests assert |

## ZS-GRAPH-006 -- Dependency-complete invalidation

| Field | Finding |
|---|---|
| STATUS | **implemented** (two layers: core graph engine + engine witness cache) |
| Requirement | On changed object/contract roots, invalidate the reflexive descendant cone for each authoritative certificate |
| Evidence | `invalidation.rs`: `upward_closure` includes changed roots + all transitive descendants (L126-140); `InvalidationCertificate {invalidated, is_upward_closure}` (L94-103); `assert_incremental_equivalence` second loop enforces soundness: any producer whose output changed but was not recomputed => `EquivalenceDivergence` (no under-invalidation, L352-364). Repo law AGENTS.md: "Sound overapprox invalidation may invalidate too much, never too little". `witness_cache.rs` `verify_entry` (root re-resolution; unverifiable root fails closed). |
| Tests | In-crate: `upward_closure_includes_transitive_dependents`, `sound_overapprox_may_over_invalidate_never_under`. Engine: `tests/engine/witness_cache_invalidation.rs`: `unrelated_file_touch_invalidates_zero_entries`, `dependency_touch_invalidates_exactly_the_affected_entry` (exact-affected assertion), `negative_answer_is_keyed_by_scope_anti_dependencies` |
| GAP | Acceptance test asks for *random DAG* tests -- existing tests are fixed DAGs/fixtures (proptest coverage exists for blast, not for the invalidation engine). Core RecomputeEngine has no random-DAG property test |
| CONFIDENCE | High -- semantics + exact-invalidation tests are strong; random-DAG test gap is the only miss |

## ZS-GRAPH-007 -- Equality-boundary early cutoff

| Field | Finding |
|---|---|
| STATUS | **partial** (cache-entry-level equality reuse exists; no graph-level boundary-artifact cutoff) |
| Requirement | Recompute boundary artifacts, stop downstream propagation when all separating boundary roots equal prior roots |
| Evidence | `witness_cache.rs` L10-20 + `verify_entry`: reuse is decided by re-resolving every recorded root (file content digests, scope listing digests) against current content; all-equal => `CacheHit` without recompute (equality-based early reuse). `CacheRoot::file`/`scope` encode exact boundary values (L70-95). `dirty_from_closure` (invalidation.rs L374) lets journal deltas stop propagation for artifacts whose consulted closure did not change |
| Tests | `tests/engine/witness_cache_invalidation.rs`: `every_warm_entry_is_reusable_before_any_edit`; `unrelated_file_touch_invalidates_zero_entries` (edit outside boundary => downstream cache preserved) |
| GAP | `RecomputeEngine.incremental_recompute` recomputes the entire affected cone unconditionally -- no equality check that stops propagation mid-cone when a boundary artifact recomputes to an identical value. "Syntactic changes yielding equal boundary values preserve downstream cache" is only satisfied indirectly (unrelated-file edits), not for an in-cone artifact whose value happens to be unchanged. No separate "boundary artifact" concept |
| CONFIDENCE | Medium-high -- equality-based reuse is real but at cache-entry granularity, not graph-cone granularity |

## ZS-GRAPH-008 -- Counterexample-guided graph refinement

| Field | Finding |
|---|---|
| STATUS | **missing** (adjacent modeling only; no capture-add-revoke-retain loop) |
| Requirement | Capture actual undeclared influences from sandbox traces, add true missing edges, revoke dependent certificates, retain counterexamples |
| Evidence | Adjacent only: `graphzero-store/src/store/claim/report.rs` `SurvivingSpan` -- "one graph-backed counterexample when a claim is false" (claim verification, not dependency refinement). `graphzero-core/src/omission.rs` `OmissionImpact` with `OmissionKind::MissingDependencyEdge` => `RecoveryTrigger::ForceAutomaticRecovery` + `blocks_candidate_publication()` (models the *need*, has no loop). `TruthClass::RuntimeObserved` exists (truth.rs L14). No sandbox-trace capture, no trace->edge add, no certificate revocation, no retained-counterexample store, no convergence test |
| Tests | None for the refinement loop |
| GAP | The entire acceptance flow (finite fixture converges after at most #missing-true-edges under fair exercise) is unimplemented. `OmissionImpact` + `SurvivingSpan` are the natural hooks |
| CONFIDENCE | High (absence confirmed by targeted grep for trace/refine/sandbox/undeclared across crates) |

## ZS-GRAPH-009 -- Cross-branch convergence

| Field | Finding |
|---|---|
| STATUS | **partial** (content-addressed CAS + branch snapshot pointers; no causal-key dedup test across branches) |
| Requirement | Share objects across branches when complete causal key + payload root match; branch name alone is not identity |
| Evidence | `graphzero-store/src/store/git.rs`: branch snapshot pointers under `branches/`, `record_head_snapshot` (L154), `branch_snapshot_ids` (L167), `dangling_branch_pointers` (L193), re-point from HEAD branch map (L339). `graphzero-store/src/store/shared_cas.rs`: content-addressed `SharedCas` (sha256-keyed `put`/`get_verified`; `open_labeled` sharing across store roots). git OID map: blobs recorded by git OID and served without local duplicate put (`indexer.rs` test `git_backed_blobs_record_oid_and_skip_local_duplicate_put`) |
| Tests | `tests/store/shared_cas_contract.rs`: `identical_bytes_deduplicate_to_one_object`, `concurrent_identical_writers_converge_on_one_valid_object`, `explicit_shared_root_dedupes_and_default_roots_stay_isolated` |
| GAP | No test builds the same branch content twice and asserts artifact dedup at the *causal key* level ("independently built identical branch artifacts deduplicate; different dependencies do not"). Byte-level CAS dedup + git OID sharing are branch-name-agnostic by construction, which implies the property for payloads, but the causal-key (dependency-set) half is unproven. Snapshot ids appear branch-scoped via branch pointers |
| CONFIDENCE | Medium -- dedup machinery is real and content-keyed; acceptance-level evidence missing |

## ZS-GRAPH-010 -- Dynamic/runtime dependency adapters

| Field | Finding |
|---|---|
| STATUS | **partial** (compiler/LSP adapters exist; no build systems/test runners/package managers/general runtime probes) |
| Requirement | Instrument build systems, test runners, interpreters, compilers, package managers, selected runtime probes to discover nonstatic dependencies |
| Evidence | `crates/graphzero-extract/src/tsserver.rs` -- tsserver typed-edge adapter (Exact/InterfaceDispatch/ReExport/Inferred confidence bands). `crates/graphzero-extract/src/rust_analyzer_lsp.rs` -- live rust-analyzer LSP subprocess resolver (spawns child, textDocument/definition RPCs; concurrency contract documented). `typed_fusion.rs` `fuse_typed_edges`. `confidence_band.rs` (TSSERVER_EXACT/INFERRED etc.) |
| Tests | `tests/extract/live_rust_analyzer_fusion.rs` (spawns real rust-analyzer), `tests/store/typed_edge_fusion.rs` (serial) |
| GAP | Covers compilers/LSP servers only. No build-system (cargo/make) instrumentation, test-runner hooks, package-manager (npm/cargo registry) dependency capture, or runtime probes. Best-effort by design (missing binary => fewer resolutions, never error). Acceptance "declared bounded domains reach BoundedComplete under adversarial dynamic access tests" not implemented: confidence bands exist, but no BoundedComplete-grade assignment for dynamic domains and no adversarial dynamic-access test |
| CONFIDENCE | High on what exists; scope gap confirmed by crate inventory |

---

## Architecture (how pieces connect)

- **Store layer** (`graphzero-store`) is the index authority: `index_repo` -> fingerprint scan -> incremental vs full collect -> delta-log/compaction -> `Snapshot` (CSR adjacency, symbol table, edge evidence spans, coverage bitmap, blob CAS). Git-backed blobs go through OID map; `SharedCas` dedups across store roots. Branch pointers (`branches/`) map branch -> snapshot id.
- **Extract layer** (`graphzero-extract`, `graphzero-scip`, `graphzero-semantic`) feeds it: tree-sitter Tier-A facts, optional live rust-analyzer/tsserver typed fusion, SCIP ingest, semantic embeddings.
- **Core contract layer** (`graphzero-core`) is the structure/truth authority: `ProjectGraph` (nodes/edges + coverage class + negative-knowledge certificates), `DependencyGraph`/`RecomputeEngine` (certified incremental invalidation), `DecisionClosure`, `WorldFiber`, `AddressAtlas`, `OmissionImpact`, `EffectConsequenceMap`/`VerifierObligationMap`.
- **Engine layer** (`graphzero-engine`) composes: blast radius + `impact_before_edit`, `rewrite_closure` (implied edit sites), `witness_cache` (minimum-dependency-set keys, env/toolchain roots, anti-dependency scope roots, completeness witness), `oracle`/`deterministic_facts` (canonical determinism), coverage-graded query results.
- **Coverage layer** (`graphzero-coverage`) gives the three-answer query model (Present/Absent/Unknown with mandatory `CoverageCertificate`), per-tier bitmaps, freshness verification.

## Start here

`/Users/aditya/AI/GraphZero/crates/graphzero-core/src/invalidation.rs` -- the single file carrying GRAPH-002/005/006/007 core semantics (DependencyGraph, RecomputeEngine, certified invalidation). Then `graphzero-store/src/store/indexer.rs` (GRAPH-001) and `graphzero-engine/src/witness_cache.rs` (GRAPH-002/007 engine-side). The hub-side crosswalk should treat GRAPH-003 as "semantic match under renamed grades, authorization boundary stricter" and GRAPH-008 as the one true gap.

## Key constraints / risks

- GraphZero AGENTS.md: incremental vs full must never diverge on protected claims; invalidation may over-approximate, never under-approximate. Any implementation of GRAPH-007/008 must keep the `assert_incremental_equivalence` fail-closed contract.
- Grade-name divergence (GRAPH-003) will block any automated conformance check that string-matches Proved/BoundedComplete/Observed/Unknown.
- GRAPH-010's typed fusion is opt-in/test-only today (documented "do not promote to default index" while single-LSP-client) -- a capacity constraint for scaling the adapters.
