# ZeroStack RACC V6 Implementation Master Plan

This plan assumes RACC-R is already partially implemented. It does not prescribe a replacement module tree. Each phase begins with an audit and maps semantic obligations onto existing code.

## Phase 0 - repository audit and baseline capture

### Construct

- repository/module map for ZeroStack, FSZero, GraphZero, and TokenZero;
- actual tool/harness entry points;
- current object identity and serialization rules;
- current cache/index/store paths;
- current execution and mutation path;
- current fallback and error behavior;
- current resource instrumentation.

### Prove or record

- which Draft 5/V6 requirements are implemented, partial, conflicting, or absent;
- exact same-model/same-harness baseline workflow;
- native-tool availability and reasoning settings;
- performance baseline for representative inspect/refactor/port/build tasks.

### Gate

No semantic implementation begins until every P0 requirement has an owner, evidence path, and dependency status.

## Phase 1 - trusted identity, contracts, and event log

### Required capabilities

- versioned canonical bytes;
- content roots with algorithm/version binding;
- rooted task/model/reasoning/harness/tool/verification contracts;
- formation receipts;
- append-only authoritative event log;
- typed `Safe`, `Unsafe`, `Unknown`;
- module/type boundary preventing untrusted authority construction.

### Gate

Golden fixtures serialize identically across processes and supported platforms. Mutation of any semantic field changes the root. Missing premises yield `Unknown`. Replay reconstructs the same authoritative state.

## Phase 2 - exact read-only Zero Execute

### Required capabilities

- harness adapter with stable semantic request/result contract;
- exact project snapshot;
- FSZero exact reads/spans/search results;
- GraphZero definitions/references/callers/tests for one language;
- task-relative causal lens with declared completeness grade;
- TokenZero stable decision capsule and expansion handle;
- continuation handle;
- complete resource ledger;
- native-tool fallback.

### Gate

For selected explanation and local-edit-preparation tasks, ZeroStack produces exact expandable evidence from the same root. Paired same-model trials show no missing factual support inside the declared scope. Provider-visible history is append-only.

## Phase 3 - L1/L2/L3 cache separation

### Required capabilities

- explicit provider-cache observations;
- L2 causal key and logical validity;
- L3 tier/residency records;
- provider-miss insulation path;
- stable first-emission capsules;
- deterministic rendering and stable tool schema;
- forced provider-miss benchmark.

### Gate

A forced L1 miss does not trigger relisting, rereading, or reindexing unchanged project state while L2 remains valid. Metrics report L1, L2, L3, model-visible context, and complete work separately.

## Phase 4 - dependency-complete causal invalidation

### Required capabilities

- versioned dependency graph;
- change-to-descendant invalidation;
- completeness grades;
- runtime dependency observation;
- equality-boundary early cutoff;
- branch/root deduplication;
- refinement counterexample log.

### Gate

Controlled mutations invalidate all and only the certified affected cone in the supported domain. Injected missing edges cause `Unknown` or a failed certificate, never a stale hit.

## Phase 5 - private mechanical composition

### Required capabilities

- trace segmentation into semantic decisions and mechanical segments;
- private-composability checker;
- contingent policy representation;
- operation DAG and critical-path ledger;
- backend execution of searches/reads/builds/tests without transcript flooding;
- return-to-model gate for unresolved decisions.

### Gate

A hidden segment never makes an unapproved semantic choice. One-/two-call coverage is measured against blinded decision-boundary annotation.

## Phase 6 - transactional edits and effects

### Required capabilities

- child sandbox;
- exact effect/read/write/process trace;
- typed edit/effect representation or exact patch preimages;
- formatter/generator-aware exact delta derivation;
- verifier plan;
- successor-state check;
- short-lived authority lease;
- expected-parent root/epoch compare-and-swap;
- exact no-mutation failure.

### Gate

Crash and race injection at every verify/authorize/commit boundary yields old root or complete verified new root, never partial mutation.

## Phase 7 - Q99 residency and eviction control

### Required capabilities

- demand-weight ledger;
- current valid demanded mass;
- L3 residency plan proposals;
- deterministic capacity/Q99 checker;
- eviction slack guard;
- windowed Q99 and restoration reporting;
- invalidation-hazard and recomputation-cost observations;
- cold-tier fetch/recompute measurement.

### Gate

No eviction can violate a declared Q99 certificate without explicit service degradation or compensating admission. Q99 claims are rejected when denominator, weights, validity, window, or tier are absent.

## Phase 8 - cross-harness conformance

### Required capabilities

- at least three adapters or transport modes;
- canonical semantic conformance vectors;
- continuation migration/compatibility checks;
- adapter-specific rendering roots;
- project-semantic object reuse across faithful adapters;
- cancellation, timeout, and fallback parity.

### Gate

The same canonical task and project root produce equivalent protected semantic outcomes and rooted backend transitions across adapters, modulo declared rendering/model contracts.

## Phase 9 - verified capabilities

### Required capabilities

- episode capture;
- capability proposal;
- exact scope/precondition/dependency/effect/verifier/rollback record;
- freshness states;
- revocation and revalidation;
- failure-syndrome storage;
- shadow execution;
- strict-rescue and lifetime-value accounting.

### Gate

No capability publishes while stale, Unknown, or unverified. Baseline remains available. Failed capture and maintenance work are fully charged.

## Phase 10 - public empirical release

### Required evidence

- paired same-model/same-harness traces;
- one-/two-call coverage;
- protected regression and strict-rescue adjudication;
- forced provider-miss trials;
- Q99 across logical, physical, provider, interface, and complete-work coordinates;
- post-change restoration curves;
- storage/CPU/RAM overhead;
- security and fault-injection results;
- negative results and failed premises;
- current provider fact sheet;
- reproducible manifests and hashes.

## Default implementation order within a phase

1. Root evidence and schema.
2. Pure deterministic checker.
3. Counterexample/Unknown behavior.
4. Audit event and ledger charge.
5. Authority consequence.
6. Unit/property tests.
7. Fault injection.
8. Paired end-to-end test.
9. Performance measurement.

Do not implement optimization before the corresponding truth/authority checker exists.
