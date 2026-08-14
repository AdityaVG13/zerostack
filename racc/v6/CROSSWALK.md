# RACC V6 Crosswalk -- 130 requirements vs the four repos

Audited 2026-08-14 against ZeroStack `48d71d3`, FSZero, GraphZero `35441f0`,
TokenZero (working trees). Eight parallel audit scouts + orchestrator audit of
ZS-ADAPTER-010/011. Full evidence (file:line per row) lives in
`distilled/xwalk_*.md`. This file is the repo-state authority; the CSV `status`
column ("Not implemented / audit required") is a corpus default, now stale.

Status taxonomy (per `01_IMPLEMENTATION_AGENT_PROMPT.md`): implemented /
partial / conflicting / missing / not applicable.

## Totals

| Status | Count |
|---|---|
| implemented | 10 |
| partial | 71 |
| conflicting | 1 |
| missing | 48 |
| **total** | **130** |

**Implemented (10):** ZS-BASE-002 (strict no-downshift reasoning audit),
ZS-STORE-001 (CAS), ZS-STORE-008 (GC/leases), ZS-GRAPH-001 (incremental index),
ZS-GRAPH-005 (executable closure checker), ZS-GRAPH-006 (dependency-complete
invalidation), ZS-VIEW-003 (exact expansion), ZS-ADAPTER-006
(cancellation/deadline propagation), ZS-SEC-003 (authority replay protection),
ZS-OPS-001 (crash recovery).

**Conflicting (1):** ZS-ADAPTER-003 -- the repo's typed envelope uses its own
failure-code vocabulary; the mandated result kinds (Completed, DecisionRequired,
EvidenceExpansionRequired, VerificationUnknown, BaselineFallbackRequired,
RejectedNoMutation, Cancelled, FailedNoAuthority) have zero occurrences in
crates/.

## Structural findings (cross-cutting)

1. **The Q99/gate/ledger layer is unwired.** `zero-gate` (q99, invalidation,
   two-phase, quality, reinvestment, deoptimization) and `zero-ledger`
   (causal_work, fresh_work) are complete, tested contract/verifier primitives
   that NO runtime depends on. zsx-core depends only on zero-abi/zero-store.
   No production path mints their receipts. This is the single highest-leverage
   wiring gap in the program.
2. **Decision boundary is a correctness hole, not a feature gap.** The
   zero-codemode interpreter privately evaluates all branches; with no
   DecisionRequired return (EXEC-003), no contingent policy (EXEC-004), and no
   semantic-decision gate (EXEC-007), hidden semantic branches CAN be privately
   selected today -- exactly what V6-C03/H03 forbid.
3. **Continuation layer is greenfield.** SESSION-001..005 + ADAPTER-004 all
   missing; `replace()` discards request history. Seeds: zero-store durable
   journals, `state_root`, worker handshake pins.
4. **No trivalent verdict type.** Zero matches for `Unsafe` repo-wide; per-domain
   `Unknown` values fail closed (spirit ok) but there is no shared
   Safe/Unsafe/Unknown type (KERNEL-004).
5. **No parent-rooted event log / project-level successor CAS.** No
   `parent_root`/`prev_root` linkage anywhere; atomicity is per-journal, not
   "verified successor root XOR unchanged" on an authoritative project root
   (KERNEL-006/008).
6. **Two parallel cache-entry implementations.** zero-abi `cache_entry.rs` vs
   GraphZero `witness_cache.rs` are wire-compatible but unshared; GraphZero's
   copy is not wired into its query paths. Drift risk.
7. **Capability subsystem (CAP-001..006, METRIC-009) is 100% spec-only.**
   Zero code in any repo.
8. **Benchmark program is mechanism-rich, trial-poor.** Verification contracts
   + unit tests exist; manifests, annotations, forced-miss matrices, windows,
   capacity curves do not. `benchmarks/` is a catalog.
9. **Grade-name divergence.** GraphZero uses
   Complete/SoundOverapproximation/ObservedOnly/Unknown, not V6's
   Proved/BoundedComplete/Observed/Unknown -- and is stricter (SoundOverapprox
   does not permit absence certification where V6's BoundedComplete does).
10. **TokenZero core view/capsule types are orphaned.** `DecisionView`,
    `ModelCapsule`, `TokenPage`, `ExactTokenMap` are implemented, digest-
    canonical, tested -- and constructed by no engine production path.

## Per-subsystem status

### Trusted kernel (8) -- 8 partial · `distilled/xwalk_kernel.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-KERNEL-001 | P0 | partial | `canonical_json` + digest layer exist; no versioned canonical-byte registry over all object classes; no boundary rejector |
| ZS-KERNEL-002 | P0 | partial | sha256 roots + domain-versioned preimages; algorithm tag not structurally bound in every root |
| ZS-KERNEL-003 | P0 | partial | CacheKeyV1 + verifier witnesses; no payload formation receipt (constructor+contract+execution+epoch) |
| ZS-KERNEL-004 | P0 | partial | per-domain Unknown fails closed; no shared Safe/Unsafe/Unknown type (`Unsafe` = 0 matches) |
| ZS-KERNEL-005 | P0 | partial | linear ExecutionPermit + zero-cert compile_fail; no planner/model/optimizer module-boundary audit |
| ZS-KERNEL-006 | P0 | partial | append-only counters + WALs; no unified parent-rooted authoritative event log |
| ZS-KERNEL-007 | P0 | partial | per-type version consts + golden fixtures + fail-closed; no single rooted ABI version, no migration receipts |
| ZS-KERNEL-008 | P0 | partial | journal 2PC + crash-boundary tests; no project-level successor-root CAS loop |

### Contracts (4) -- 4 partial · `distilled/xwalk_kernel.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-CONTRACT-001 | P0 | partial | CwirTaskContractV1 = kind + opaque digests; criteria/protected dims/budget/fallback not structured |
| ZS-CONTRACT-002 | P0 | partial | ReasoningContractV1 strong; no stopping policy / system-prompt root / sampling fields |
| ZS-CONTRACT-003 | P0 | partial | AdapterBinding digests only; no harness contract (serialization/ordering/transcript/cancel) |
| ZS-CONTRACT-004 | P0 | partial | CwirCoverageV1 coarse enum; no protected-scope obligation type |

### Baseline sovereignty (3) -- 1 implemented, 2 partial · `distilled/xwalk_kernel.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-BASE-001 | P0 | partial | native in-process path + no-spawn guards; no disable-mid-run conformance task |
| ZS-BASE-002 | P0 | **implemented** | `verify_strict_no_downshift_v1` + full downshift-rejection tests |
| ZS-BASE-003 | P0 | partial | raw-baseline cost accounting + Unknown refuses speculation; no reserve pool + late-failure test |

### Harness adapters (11) -- 1 implemented, 5 partial, 1 conflicting, 4 missing · `distilled/xwalk_adapter_exec.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-ADAPTER-001 | P0 | partial | fixed zsx envelope/CLI/N-API; not a registered model tool; 10k schema-identity test absent |
| ZS-ADAPTER-002 | P0 | partial | typed transports; no canonical Task Contract; no cross-transport round-trip fixture |
| ZS-ADAPTER-003 | P0 | **conflicting** | own envelope vocabulary vs mandated 8-outcome result kinds (0 matches) |
| ZS-ADAPTER-004 | P0 | missing | no continuation handle type at all |
| ZS-ADAPTER-005 | P0 | partial | additive by construction; no coexistence test |
| ZS-ADAPTER-006 | P1 | **implemented** | CancellationSignal + deadline_unix_ms + journaled Indeterminate recovery + tests |
| ZS-ADAPTER-007 | P1 | partial | 12KiB inline budget + CAS spill + step receipts; no streaming/progress channel |
| ZS-ADAPTER-008 | P1 | missing | no semantic/render root split |
| ZS-ADAPTER-009 | P2 | missing | component tests only; no same-task cross-adapter golden suite |
| ZS-ADAPTER-010 | P0 | partial | stable envelope by construction; zero-mcp catalog is dynamic; no schema-stability fixture |
| ZS-ADAPTER-011 | P1 | missing | per-transport fixtures exist; no 3-transport semantic replay |

### Session/continuation (5) -- 5 missing · `distilled/xwalk_adapter_exec.md`

All of ZS-SESSION-001..005 missing: no continuation root, no state machine with
transition table, no branching, no compaction API, no durable handle
compatibility. Seeds: durable journals, state_root, handshake pins,
`SessionReplacementReceipt`.

### Private composition / exec (7) -- 3 partial, 4 missing · `distilled/xwalk_adapter_exec.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-EXEC-001 | P0 | missing | scripts not rooted DAGs; no decision-boundary node marking (fs.plan lives in FSZero) |
| ZS-EXEC-002 | P0 | partial | private composition real (bounded interpreter, public-result normalization, spill refs); DAG framing + trace-equivalence test absent |
| ZS-EXEC-003 | P0 | missing | no DecisionRequired return path |
| ZS-EXEC-004 | P1 | missing | no contingent policy type |
| ZS-EXEC-005 | P1 | partial | queue-level parallelism + per-engine serialization; no critical-path DAG scheduling |
| ZS-EXEC-006 | P1 | partial | registry + effect classes + handshake pins; build/test/package/profiler adapters engine-side |
| ZS-EXEC-007 | P0 | missing | no semantic-decision gate -- hidden branches privately selectable today |

### Object store / FSZero (9) -- 2 implemented, 7 partial · `distilled/xwalk_store.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-STORE-001 | P0 | **implemented** | SharedCas: atomic publish, verify-on-read, no-overwrite, quarantine; 29 tests green |
| ZS-STORE-002 | P0 | partial | ExactSnapshot files-only; no modes/symlinks/lockfiles/toolchain/external-input coverage |
| ZS-STORE-003 | P0 | partial | EvidencePage fails closed (StaleSource/DigestMismatch); no unified span resolver; CRLF not canonicalized |
| ZS-STORE-004 | P0 | partial | overlays + path guards + immutable base; no OS sandbox, no process/network/env effect tracking |
| ZS-STORE-005 | P0 | partial | minimal-span deltas + deterministic reseal; no full-workspace independent rescan; no effect receipts |
| ZS-STORE-006 | P0 | partial | expected-parent CAS in durable journal (RootMismatch); five-term binding (scope+nonce+lease) not unified |
| ZS-STORE-007 | P1 | partial | verify-on-access + repair + quarantine + SQLite gate; no scrubber, no replication, no L2/L3 model |
| ZS-STORE-008 | P1 | **implemented** | lease/pin/reachability GC, fault-tested sweep resume; reachability producer-declared |
| ZS-STORE-009 | P1 | partial | project-key namespacing + per-repo DBs; no encryption, no authorization, dedup side channel open |

### GraphZero (10) -- 3 implemented, 6 partial, 1 missing · `distilled/xwalk_graph.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-GRAPH-001 | P0 | **implemented** | incremental==full content-signature parity tested; generated-artifact/ownership edges unpopulated |
| ZS-GRAPH-002 | P0 | partial | DependencyGraph + witness-cache env/toolchain roots; no network-fixture/clock/randomness deps |
| ZS-GRAPH-003 | P0 | partial | full lattice semantics under different names; stricter than V6; no upgrade/revocation API |
| ZS-GRAPH-004 | P0 | partial | atlas + DecisionClosure + blast + rewrite_closure; no composed task-lens entry point |
| ZS-GRAPH-005 | P0 | **implemented** | RecomputeEngine fixed-point + incremental-equivalence fail-closed |
| ZS-GRAPH-006 | P0 | **implemented** | upward closure + sound-overapprox law + exact-invalidation tests; random-DAG property test absent |
| ZS-GRAPH-007 | P1 | partial | equality reuse at cache-entry granularity; no in-cone boundary cutoff |
| ZS-GRAPH-008 | P1 | missing | no counterexample-guided refinement loop (OmissionImpact/SurvivingSpan are hooks) |
| ZS-GRAPH-009 | P1 | partial | CAS dedup + branch pointers; causal-key-level dedup unproven |
| ZS-GRAPH-010 | P2 | partial | tsserver/rust-analyzer adapters; no build/test/package/runtime probes |

### TokenZero / views (10) -- 1 implemented, 8 partial, 1 missing · `distilled/xwalk_view.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-VIEW-001 | P0 | partial | DecisionView rooted + deterministic; no candidate-choices field; orphaned from production |
| ZS-VIEW-002 | P0 | partial | ModelCapsule/TokenPage exist; no causal key/formation receipt; no engine constructs them |
| ZS-VIEW-003 | P0 | **implemented** | digest-verified expand, stale-ref explicit, typed miss; secret-masking documented exception |
| ZS-VIEW-004 | P0 | partial | versioned renderer contract + structural determinism; no permutation/fuzz proof; single-contract |
| ZS-VIEW-005 | P0 | partial | append-only ledgers/exposure; no provider-message history policy or LCP acceptance test |
| ZS-VIEW-006 | P1 | partial | byte-threshold capsule admission; no horizon/expansion-probability model |
| ZS-VIEW-007 | P1 | partial | stable-before-volatile structurally enforced; no survival scores, no permutation tests |
| ZS-VIEW-008 | P1 | partial | tokenizer families are disclosed estimates; exact adapter test-only; no provider golden fixtures |
| ZS-VIEW-009 | P1 | missing | sufficiency proof is hub/GraphZero territory; TokenZero correctly never upgrades Unknown |
| ZS-VIEW-010 | P0 | partial | evidence roots + expansion handles + fail-loud digests; no supported-decisions/grade/baseline-escape fields |

### Causal cache / Q99 (15) -- 8 partial, 7 missing · `distilled/xwalk_cache.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-CACHE-001 | P0 | partial | 9-coordinate CacheCoordinateV1 + validity enum; no L1/L2/L3 runtime measurement |
| ZS-CACHE-002 | P0 | partial | ProviderCacheTelemetry presence-sensitive (Unknown != 0); no TTFT/route/key; TokenZero-only |
| ZS-CACHE-003 | P0 | partial | FreshWorkVector exact sums; token-level, not per-object demanded sets |
| ZS-CACHE-004 | P0 | missing | no sliding windows, no Q99-unavailable state |
| ZS-CACHE-005 | P1 | missing | no restoration threshold / invalid-mass-over-time |
| ZS-CACHE-006 | P1 | partial | integer cache-crossover policy exists; no horizon inputs; no call site |
| ZS-CACHE-007 | P1 | missing | no frontier planner |
| ZS-CACHE-008 | P1 | partial | cachezero shadow mode + frecency + tombstone eviction; no prefetch/hazard |
| ZS-CACHE-009 | P2 | missing | no zero-failure sample bound (299-trial) code |
| ZS-CACHE-010 | P0 | missing | Q99 claims are scalar; no demanded-object weights/window/tier ledger |
| ZS-CACHE-011 | P0 | missing | no residency-plan type or independent capacity checker |
| ZS-CACHE-012 | P0 | missing | eviction mechanics exist; no slack (sigma) computation |
| ZS-CACHE-013 | P0 | partial | attempt recovery + ActionCache; tombstones destroy L2 validity on L3 loss |
| ZS-CACHE-014 | P1 | partial | complete-work claim shape + tests in zero-gate; no runtime measurement |
| ZS-CACHE-015 | P1 | partial | branch-neutral keys by construction; no tenancy; GraphZero duplicate contract |

### Verification (7) -- 5 partial, 2 missing · `distilled/xwalk_verify_sec_ops.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-VERIFY-001 | P0 | partial | trusted-lock identity binding strong; no registry record with runtime/grade |
| ZS-VERIFY-002 | P0 | partial | digest-bound delta verification; no static/security obligations; no delta-substitution fixture |
| ZS-VERIFY-003 | P0 | missing | successor_root is chaining, not successor-state verification |
| ZS-VERIFY-004 | P0 | partial | verdict vocabulary distributed (recovery/quality); no unified Equivalent/Dominates/Reject/Unknown API |
| ZS-VERIFY-005 | P0 | partial | G0-G7 gates + grant validation; permit lacks expiry/epoch/caller identity |
| ZS-VERIFY-006 | P1 | missing | no DecisionRequired/UserApproval evaluator gate |
| ZS-VERIFY-007 | P1 | partial | causal bindings as validation contract; no live proof cache with early cutoff |

### Security (5) -- 1 implemented, 2 partial, 2 missing · `distilled/xwalk_verify_sec_ops.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-SEC-001 | P0 | partial | process-tree isolation + rlimits + identity; no fs/network/device confinement |
| ZS-SEC-002 | P0 | partial | canonical-encoding + tamper tests; tenant half untestable (no tenancy) |
| ZS-SEC-003 | P0 | **implemented** | single-use grants, expiry, binding, generation CAS, replay-safe tests |
| ZS-SEC-004 | P1 | missing | no redaction anywhere (only 2 env vars stripped) |
| ZS-SEC-005 | P0 | missing | CAS open by digest; no project authorization scoping |

### Operations (6) -- 1 implemented, 4 partial, 1 missing · `distilled/xwalk_verify_sec_ops.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-OPS-001 | P0 | **implemented** | crash boundaries at every journal state, deterministic recovery, never-redispatch |
| ZS-OPS-002 | P1 | partial | generation CAS + per-engine serialization + store locks; no branch/snapshot isolation |
| ZS-OPS-003 | P1 | partial | telemetry structs + sealed receipts; no export pipeline or sealed manifests |
| ZS-OPS-004 | P1 | missing | version rejection only; no migration transformations/receipts |
| ZS-OPS-005 | P2 | partial | worker identity + frame/grant validation; remote-worker threat untested |
| ZS-OPS-006 | P0 | partial | 96-256MB RSS + 300s CPU policies; no <=0.1% CPU / <=500MB idle gate |

### Accounting / Pareto (11) -- 5 partial, 6 missing · `distilled/xwalk_metric_bench_cap.md`

| ID | P | Status | One-line |
|---|---|---|---|
| ZS-METRIC-001 | P0 | partial | TokenLedger + causal-work classes + pulse; no bytes/CPU/GPU/storage; no bill reconciliation |
| ZS-METRIC-002 | P0 | missing | no paired baseline manifest (DistributionalClaimV1 is a consumer) |
| ZS-METRIC-003 | P0 | partial | 3 labeled Q99 claims; other 5 metrics + dashboard absent |
| ZS-METRIC-004 | P0 | missing | no feasibility solver |
| ZS-METRIC-005 | P1 | missing | digest field only; no amortization computation |
| ZS-METRIC-006 | P1 | partial | fresh/replayed/recovery/overhead decomposition; not the 3-term frontier form |
| ZS-METRIC-007 | P1 | missing | no disjoint charging maps / Gamma |
| ZS-METRIC-008 | P1 | partial | reinvestment vectors + componentwise budget refusal implemented; savings provenance unlinked |
| ZS-METRIC-009 | P1 | missing | no capability lifetime ledger |
| ZS-METRIC-010 | P1 | missing | no certified lower bound |
| ZS-METRIC-011 | P0 | partial | deoptimization/baseline-safepoint machinery; no explicit sovereignty-audit invalidation rule |

### Benchmarks (12) -- 3 partial, 9 missing · `distilled/xwalk_metric_bench_cap.md`

BENCH-004 partial (invalidation mechanism + contract vectors; no trial),
BENCH-006 partial (journal fault matrix in testkit; not the full 20-fault
program), BENCH-008 partial (quality certificate ABI; no evaluation pipeline).
BENCH-001/002/003/005/007/009/010/011/012 missing -- the empirical program has
no harness: no task manifests, no annotations, no forced-miss experiments, no
windows, no capacity curves, no release claim gate.

### Capabilities (6) -- 6 missing · `distilled/xwalk_metric_bench_cap.md`

ZS-CAP-001..006 all missing. Zero code; spec-only (V6 canonical spec section 15).

## Priority clusters (dependency-ordered)

1. **W0 -- Result vocabulary + decision gate** (ADAPTER-003, EXEC-003/004/007,
   VERIFY-004/006, KERNEL-004): shared trivalent verdict + 8-kind result
   envelope + DecisionRequired return. Correctness boundary; unblocks the ABI.
2. **W1 -- Identity kernel completion** (KERNEL-001/002/003/006/007/008,
   CONTRACT-001/003/004): formation receipts, parent-rooted event log,
   project-level successor CAS, structured task/harness/protected-scope
   contracts.
3. **W2 -- Wire the gate/ledger layer** (CACHE-014, METRIC-001/003/011,
   VERIFY-001/005/007): make zsx-core/engines produce zero-gate/zero-ledger
   receipts on real dispatches.
4. **W3 -- Continuation layer** (SESSION-001..005, ADAPTER-004, ADAPTER-010
   hardening): handles, state machine, branching, durable compatibility.
5. **W4 -- Q99 runtime** (CACHE-001/004/005/010/011/012, GRAPH-007 cutoff,
   STORE-007 L2/L3): demand-weight ledger, windows, residency plans + slack
   guard, layer accounting.
6. **W5 -- Sandbox + tenancy + secrets** (STORE-004, SEC-001/004/005,
   STORE-009): effect tracing, confinement, redaction, project authorization.
7. **W6 -- Benchmark + release program** (BENCH-001..012, METRIC-002/004/005,
   OPS-003/006 gate, RELEASE-001 checker): paired manifests, fault program,
   forced-miss matrices, claim gates.
8. **W7 -- Capabilities** (CAP-001..006, METRIC-009/010): shadow-mode first;
   last because it depends on every preceding truth/authority layer.
9. **Cross-repo hygiene**: unify cache-entry contract (zero-abi vs GraphZero),
   decide V6-vs-GraphZero grade-name mapping, wire TokenZero DecisionView/
   ModelCapsule into an engine surface, add the 4 unmapped process theorems to
   the theorem-to-runtime map.
