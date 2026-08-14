# Implementation Backlog V6 Summary

Total requirements: **130**. Draft 5 requirements are preserved verbatim and 20 Draft 6 requirements are appended.

## Priority counts

- P0: 77
- P1: 47
- P2: 6

## Subsystem counts

- Accounting / Pareto: 11
- Baseline sovereignty: 3
- Benchmarks: 12
- Causal cache / Q99: 15
- Contracts: 4
- GraphZero / indexing: 10
- Harness adapters: 11
- Object store / FSZero: 9
- Operations: 7
- Private composition / Code Mode: 7
- Security: 5
- Session/continuation: 5
- TokenZero / decision views: 10
- Trusted kernel: 8
- Verification: 7
- Verified capability: 6

## Draft 6 additions

- `ZS-SESSION-005` (P0): **Durable continuation compatibility** - Bind every continuation handle to ABI, task, project, security, model/harness rendering, and epoch roots. Resolve through explicit migration or fail closed; a handle is never authority.
- `ZS-ADAPTER-010` (P0): **Stable Zero Execute surface** - Expose one small stable semantic tool surface whose schema does not change with project contents. Dynamic state is carried through rooted arguments, capsules, and continuation handles.
- `ZS-ADAPTER-011` (P1): **Cross-adapter semantic replay** - Replay canonical task/result vectors through at least three transports and verify equivalent protected semantic roots, cancellation, timeout, Unknown, fallback, and ledger behavior.
- `ZS-VIEW-010` (P0): **Decision-view completeness witness** - Every compact view must identify the protected decisions it supports, exact evidence roots, omitted classes, expansion handles, completeness grade, and baseline escape.
- `ZS-EXEC-007` (P0): **Unresolved semantic decision gate** - Detect observation-contingent branches not covered by a user/model contingent policy or a uniquely resolving verifier and return DecisionRequired before authority is exercised.
- `ZS-CACHE-010` (P0): **Demand-weight ledger** - Record the exact demanded object closure and positive per-object demand weights for each declared Q99 coordinate and window. Denominators include misses and Unknown objects.
- `ZS-CACHE-011` (P1): **Residency-plan proposal and verifier** - Allow heuristics/solvers to propose L3 resident sets, but independently verify capacity and retained valid demand mass before issuing a tier-specific Q99 certificate.
- `ZS-CACHE-012` (P0): **Q99 eviction slack guard** - Before eviction, compute current resident valid demand mass, Q99 slack, proposed removed mass, and post-eviction certificate. Deny or compensate any eviction that violates a declared service threshold.
- `ZS-CACHE-013` (P0): **L2-valid/L3-cold recovery** - Distinguish logically valid objects from physical residency. An L3 miss fetches or deterministically rematerializes the valid L2 object without relisting/reindexing unchanged project state.
- `ZS-CACHE-014` (P1): **Provider-miss bounded amplification metric** - Measure baseline replay burden B, compact decision-view burden C, continuation overhead L, and complete backend work after forced provider misses; report 1-(C+L)/B only for the model-visible coordinate.
- `ZS-CACHE-015` (P1): **Cross-session and branch deduplication** - Reuse exact L2 objects across sessions, branches, and faithful harness adapters by canonical content/contract/dependency identity while preserving security and tenancy authorization.
- `ZS-METRIC-010` (P1): **Certified unavoidable-work lower bound** - Represent request, irreducible decision, protected reasoning, verification, output, and external-effect lower-bound components with scope and nonoverlap evidence.
- `ZS-METRIC-011` (P0): **Reasoning allowance sovereignty audit** - Capture baseline and treatment reasoning settings, tool access, context escape, stopping policy, and fallback reserve. Prevent claims of no degradation when treatment removes any baseline reasoning or tool strategy.
- `ZS-BENCH-010` (P0): **Adaptive decision-depth annotation** - Blindly annotate baseline traces for genuine observation-contingent semantic decisions and compare with Zero Execute call count and hidden-segment classifications.
- `ZS-BENCH-011` (P1): **Residency capacity/Q99 curve** - Measure minimum RAM/disk capacity required to retain 99% valid demanded mass under observed workloads; compare LRU, LFU, size-aware, causal-weighted, and oracle plans.
- `ZS-BENCH-012` (P1): **L1/L2/L3 forced-miss matrix** - Independently force provider-prefix miss, local-residency miss, logical invalidation, and combinations; measure rediscovery, transfer, rematerialization, model-visible tokens, and complete work.
- `ZS-OPS-006` (P0): **Invisible steady-state budget** - Measure and enforce default idle/background targets near <=0.1% CPU and <=500 MB resident memory; schedule heavier indexing explicitly and disclose alternate modes.
- `ZS-SEC-005` (P0): **Cross-project cache isolation** - Separate semantic deduplication from authorization. Content-equivalent objects may share physical storage, but handles, reads, capabilities, and receipts are scoped to authorized tenants/projects.
- `ZS-CAP-006` (P1): **Capability lifetime and Q99 interaction** - Charge capability capture, proof, maintenance, residency, invalidation, and revalidation; prioritize only assets whose verified lifetime value and causal reuse justify cost.
- `ZS-RELEASE-001` (P0): **Versioned cumulative claim authority** - Ship the current correction/supersession table and reject runtime or documentation claims that cite a superseded formulation as current authority.
