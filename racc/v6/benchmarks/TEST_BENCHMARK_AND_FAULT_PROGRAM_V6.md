# ZeroStack V6 Test, Benchmark, and Fault Program

## 1. Paired experimental rule

Every efficacy or Pareto trial freezes:

- model and version;
- harness and version;
- system/developer prompt;
- reasoning allowance and stopping policy;
- initial project root;
- native tools;
- task request and acceptance criteria;
- environment and toolchain where reproducibility is claimed.

Baseline uses ordinary tools. Treatment adds optional ZeroStack. Randomized trials use paired seeds or repeated samples and disclose stochasticity.

## 2. Task families

### Inspect/explain

- unfamiliar small repository;
- medium multi-module service;
- monorepo with generated/configuration code;
- deliberately misleading names;
- runtime behavior that differs from static appearance.

### Refactor

- symbol/API rename;
- module extraction;
- persistence boundary separation;
- error-handling redesign;
- cross-cutting security change;
- performance refactor;
- refactor with one genuine architecture decision;
- refactor with multiple contingent failures.

### Port

- Python to C++ library;
- CLI tool with file/process effects;
- numerical code with tolerance obligations;
- async/network service;
- package/build-system migration.

### Greenfield

- small game;
- multi-scene game with save state;
- web/service application;
- project with subjective design dimensions requiring human acceptance.

## 3. Primary measurements

- model-visible tool calls;
- tool argument tokens;
- tool result tokens;
- total uncached input;
- provider cached input;
- hidden reasoning when observable;
- output tokens;
- backend logical operations;
- physical reads/writes and bytes;
- CPU/GPU time;
- indexing and verification work;
- latency and critical path;
- storage and memory;
- L2 logical reuse;
- L3 residency;
- invalidated causal mass;
- time/work to restore Q99;
- protected success/regression/strict rescue;
- fallback and Unknown rates;
- unresolved decision depth.

## 4. One-/two-call study

Independent annotators mark every baseline observation at which a competent model could make two protected-distinct choices. Compare the annotated adaptive decision depth with actual treatment calls.

Failure conditions:

- ZeroStack privately chooses an uncovered semantic branch;
- a supposedly mechanical segment contains model-reserved judgment;
- call compression is achieved by removing evidence or reasoning opportunity;
- model must issue primitive recovery calls because the initial lens was incomplete.

## 5. Provider-miss insulation study

1. Warm L2 and, when possible, L1.
2. Force or wait for L1 miss while preserving project root and L2.
3. Run the same task.
4. Measure compact-view reprocessing, L2/L3 work, and project rediscovery.
5. Repeat after L3 coldness with L2 valid.
6. Repeat after genuine project invalidation.

Report all three cases separately.

## 6. Causal invalidation and Q99 study

Mutation classes:

- comment/format only;
- local implementation preserving interface;
- public API change;
- build/configuration change;
- dependency version change;
- generated-file change;
- central schema/protocol change;
- branch divergence/convergence;
- hidden runtime dependency inserted as a fault.

For each mutation, record demanded closure, initial invalidated cone, equality cutoffs, recomputed mass, valid reused mass, L3 fetches, and Q99 restoration.

## 7. Residency/eviction study

Compare:

- LRU;
- LFU;
- size-aware recency;
- demand-weighted causal plan;
- hazard/recomputation-aware plan;
- oracle retrospective plan.

Every plan is independently checked for capacity and Q99. Report optimization time and checker time separately.

## 8. Fault injection

Mandatory faults:

- noncanonical encoding;
- digest/payload mismatch;
- incomplete formation receipt;
- missing dependency edge;
- stale project root;
- expired/replayed authority lease;
- cross-project handle reuse;
- provider prefix rewrite;
- L2 corruption;
- L3 object loss;
- sandbox escape attempt;
- undeclared file/process/network effect;
- verifier timeout/disagreement;
- crash before, during, and after commit CAS;
- concurrent competing commits;
- fallback reserve exhaustion;
- capability stale after drift;
- adapter truncation or field reordering;
- resource-ledger omission.

Expected result is `Unsafe`, `Unknown`, exact no-mutation, or baseline fallback. Never silent success.

## 9. Public claim gates

No public Q99 or no-degradation claim without:

- exact metric definition;
- same-scope denominator;
- paired baseline;
- current implementation commit;
- sealed resource ledgers;
- negative results;
- fault-injection evidence;
- provider facts dated and sourced;
- code and data sufficient to reproduce calculations;
- independent review of protected criteria.
