# ZeroStack RACC V6 Canonical System Specification

**Author:** Aditya G  
**Date:** 14 August 2026  
**Status:** Draft 6; canonical implementation semantics for this release

## 1. Purpose

ZeroStack is a persistent backend for agent harnesses. It reduces the model-visible and repeated complete cost of project work by moving exact project state, indexing, mechanical tool composition, verification, and reusable capability behind a stable programmatic tool boundary.

The model is unchanged. It remains responsible for semantic interpretation, architecture, tradeoffs, and any choice not uniquely resolved by the task contract or a sound verifier.

The target interaction is:

```text
model / harness
    -> Zero Execute(task, project, constraints, continuation?)
    -> ZeroStack privately resolves exact state and mechanical work
    -> Completed | DecisionRequired | EvidenceExpansionRequired |
       VerificationUnknown | BaselineFallbackRequired | RejectedNoMutation
```

A prepared task may complete in one call. A task with one unresolved semantic decision may require two calls. The lower bound is the number of unresolved adaptive decisions, not the number of files or primitive operations.

## 2. Paired baseline and protected order

For task `x`, the baseline is:

```text
B(x) = same model + same harness + same model settings + ordinary native tools
```

The treatment is:

```text
Z(x) = B(x) + optional ZeroStack strategy
```

The treatment must preserve a trace-capable injection of baseline strategies:

```text
Pi_B(x) -> Pi_Z(x)
```

The protected result relation `<=_x` is defined by the task contract. It may include exact equivalence, tests, build, API behavior, security, performance, output format, human approval, or other scoped criteria. A result may publish only when it is baseline-equivalent, protected-dominating, or accepted by the designated human authority. Otherwise the backend expands evidence or executes the baseline path.

## 3. Four-repository authority split

### 3.1 ZeroStack

Owns:

- semantic ABI and versioning;
- task contracts;
- harness adapters and `Zero Execute`;
- orchestration and continuation state;
- Safe/Unsafe/Unknown authority boundary;
- baseline reserve and fallback;
- private mechanical composition;
- verifier coordination;
- commit authority;
- resource ledger;
- Q99 reporting;
- capability registry;
- conformance and release gates.

### 3.2 FSZero

Owns:

- canonical immutable object storage;
- exact source bytes;
- snapshots and roots;
- exact spans and preimages;
- child sandboxes;
- read/write/effect traces;
- exact delta derivation;
- rollback;
- expected-parent atomic commits;
- physical materialization and storage integrity.

### 3.3 GraphZero

Owns:

- syntax and symbol indexes;
- definitions, references, callers, imports, tests;
- build and configuration dependency edges;
- control/dataflow adapters where supported;
- task-relative demanded closures;
- causal lenses;
- completeness grades and counterexamples;
- incremental graph updates and invalidation cones.

### 3.4 TokenZero

Owns:

- stable tool schema and canonical model rendering;
- decision capsules;
- expandable exact references;
- model-visible argument/result accounting;
- provider prefix-cache observations;
- deterministic ordering and fragmentation control;
- transcript and token ledgers.

Domain engines do not call one another. ZeroStack composes them through versioned rooted contracts.

## 4. Trusted semantic objects

The implementation must have semantic equivalents of the following. Names may differ; meaning may not.

### Identity and contracts

- `CanonicalRoot`
- `AbiVersion`
- `ModelRuntimeContract`
- `ReasoningContract`
- `HarnessContract`
- `ToolContract`
- `VerificationContract`
- `ProtectedScope`
- `TaskContract`

### Project state

- `ProjectRoot`
- `ProjectSnapshot`
- `ExactObjectRef`
- `ExactSpan`
- `DependencyRootSet`
- `CausalGraphRoot`
- `CausalLens`
- `CompletenessCertificate`

### Harness continuation

- `RequestEnvelope`
- `ContinuationHandle`
- `DecisionView`
- `StableCapsule`
- `ExpansionHandle`
- `ActionTable`
- `ZeroExecuteRequest`
- `ZeroExecuteResult`

### Execution and authority

- `SandboxRoot`
- `ExecutionPlan`
- `ExecutionTrace`
- `ExactDelta`
- `VerificationReceipt`
- `SuccessorCertificate`
- `BaselineAuthority`
- `AuthorityLease`
- `CommitReceipt`

### Cache and accounting

- `CausalCacheKey`
- `FormationReceipt`
- `LogicalValidityRecord`
- `ResidencyRecord`
- `ProviderCacheObservation`
- `ResourceLedger`
- `Q99Report`
- `ParetoCertificate`

### Learning and capability

- `VerifiedEpisode`
- `CapabilityAsset`
- `FailureSyndrome`
- `FreshnessState`
- `RevocationReceipt`

## 5. Authoritative state machine

```text
Idle
  -> Bound(task contract)
  -> Snapshotted(exact project root)
  -> Resolved(causal state and cache)
  -> ViewReady(protected decision view)
  -> ModelDecision? (only when required)
  -> Planned
  -> Sandboxed
  -> Executed
  -> DeltaSealed
  -> Verified
  -> Authorized
  -> Committed | NoMutation | BaselineRestored
  -> LedgerSealed
```

Every transition is recorded in an append-only rooted event log. A crash, cancellation, timeout, stale root, expired lease, verifier disagreement, or compare-and-swap race leaves the authoritative project root unchanged or restores an exactly related state.

## 6. Safe, Unsafe, and Unknown

Every checker returns one of:

```text
Safe(certificate)
Unsafe(counterexample)
Unknown(missing or unverifiable premise)
```

`Unknown` is a first-class result. It may cause:

- evidence expansion;
- graph refinement;
- stronger verification;
- model decision;
- native-tool execution;
- exact baseline fallback.

It may not issue commit or publication authority.

## 7. Decision-boundary compression

A conventional harness trace can be decomposed as:

```text
d0, sigma1, d1, sigma2, ..., sigmam, dm
```

where `di` are semantic decisions and `sigmai` are mechanical or independently verifiable operations. ZeroStack may privately compose `sigmai` only when:

1. it begins from the same rooted state;
2. the hidden operations contain no model-reserved choice;
3. the resulting decision view is protected-sufficient;
4. exact expansion remains available;
5. effects are isolated or independently authorized;
6. failures return to the model or baseline.

If `D(x)` unresolved adaptive decisions remain after the initial request and any supplied contingent policy, the prepared model-visible call count is:

```text
N_Z(x) = D(x) + 1
```

This is both the target and the lower bound in the stated decision model.

## 8. Stable decision views

A decision view contains only information needed for the next protected decision, plus exact expansion handles. It may include:

- task objective and acceptance criteria;
- project root and causal-lens root;
- relevant exact source spans;
- symbol/caller/test summaries with rooted references;
- unresolved choices;
- candidate plan or delta summary;
- verification results;
- resource receipt;
- continuation handle.

A view is not safe because it is short or semantically plausible. It is safe only when a certificate establishes that every baseline strategy relevant to the next protected decision remains reconstructible or the model can expand to exact evidence before authority is exercised.

## 9. Causal Cache Normal Form

A large artifact is represented before first model exposure as:

```text
complete causal key
+ formation receipt
+ immutable exact payload/root
+ stable model-visible capsule
+ exact expansion authority
```

The capsule is emitted once. Later expansion appends evidence and never rewrites the old capsule. This removes ZeroStack's self-induced historical-prefix rewrite component while preserving exact recovery.

## 10. Three cache layers

### L1 - provider prefix cache

A volatile accelerator over exact model-request prefixes. It is provider-controlled and may depend on prefix identity, breakpoints, model, routing, residency, and policy.

### L2 - ZeroStack logical causal cache

The authority layer. An L2 object is reusable when its canonical key, constructor/contract, complete dependency roots, formation receipt, and exact payload identity validate.

### L3 - physical materialization cache

RAM, local disk, remote object storage, or replicas holding valid L2 objects. L3 affects latency and transfer, not logical truth.

A provider miss may make the compact decision view cold at L1. It must not cause project rediscovery when L2 remains valid. An L3 miss may require fetching or reconstructing a valid L2 object; it does not make the object semantically stale.

## 11. Causal invalidation

For changed project nodes `Delta`, GraphZero invalidates the dependency-complete descendant cone. Objects outside the cone retain their causal keys. If recomputed boundary objects have identical exact roots, invalidation propagation stops at that equality boundary.

Graph completeness is graded:

- `Proved`
- `BoundedComplete`
- `Observed`
- `Unknown`

Only the declared grade may be used. A runtime-observed undeclared influence revokes dependent certificates and becomes a refinement counterexample.

## 12. Q99 definitions

A Q99 claim must name:

- numerator and denominator;
- resource coordinate;
- task population;
- demand weights;
- measurement window;
- validity and residency semantics;
- confidence or exactness status.

Examples:

- L2 valid demanded mass reuse >=99%;
- L3 locally resident demanded mass >=99%;
- provider cached input tokens >=99%;
- model-visible project-context reduction >=99%;
- complete-work reduction >=99%.

These are not interchangeable.

## 13. Transactional effects

All candidate mutations occur in an exact child sandbox. The backend records actual reads, writes, processes, network/environment observations, generators, formatters, build steps, and tests. The final exact delta is derived after execution.

Commit authority binds:

- parent root;
- epoch;
- task and protected-scope roots;
- exact delta root;
- verification receipt;
- successor certificate;
- expiration/nonce;
- allowed write/effect scope.

Compare-and-swap publishes the complete successor or nothing.

## 14. Reasoning sovereignty

The treatment's maximum reasoning allowance must be at least the baseline allowance. The model may voluntarily finish early. The backend may not force a smaller reasoning envelope, remove native tools, conceal a necessary decision, or spend the reserve required for fallback.

Identical hidden reasoning traces are not claimed unless a model/runtime contract supports exact deterministic replay. The core guarantee is preserved reasoning authority and baseline strategy availability.

## 15. Verified capability accumulation

Accepted episodes may propose reusable capabilities. A capability becomes executable only when it has:

- exact scope;
- rooted preconditions;
- declared reads/writes/effects;
- deterministic or controlled execution semantics;
- postcondition and verifier;
- successor-safety obligation;
- fallback and rollback;
- freshness and revocation policy;
- complete maintenance and capture-cost accounting.

Capabilities are optional strategies behind the same baseline firewall.

## 16. Operational constraints

The steady-state backend should be invisible when idle:

- no spawn-per-call worker model;
- no random or source-checkout worker selection;
- no silent fallback;
- no mixed-generation rollback;
- no semantic identity based on paths, timestamps, or row IDs;
- typed actionable errors;
- separate stable payload and volatile receipt fields;
- target <=0.1% background CPU and <=500 MB resident memory for the default local mode, with heavier analysis explicitly scheduled and measured.

## 17. Current endpoint

The intended result is not universal one-token magic. It is a backend that turns repeated discovery and tool plumbing into reusable, exact, privately composed project operations, leaving the model with the irreducible decisions and full ability to escape to baseline tools.
