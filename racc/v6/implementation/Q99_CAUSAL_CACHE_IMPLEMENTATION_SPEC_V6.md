# Q99 Causal Cache Implementation Specification - Draft 6

## 1. Purpose

The cache must preserve exact reusable project knowledge across provider expiry, turns, sessions, branches, and harnesses without treating stale state as valid. The implementation separates logical validity, physical residency, and provider prefix reuse.

## 2. Causal object identity

Every derived object is keyed by canonical roots over:

- constructor identity and version;
- complete semantic contract;
- dependency roots and relevant environment observations;
- canonical parameters;
- toolchain/build/runtime identity where relevant.

A formation receipt binds the key to the actual payload root and execution record. A matching key without a valid formation receipt is insufficient.

## 3. Stable first emission

Large tool results are stored exactly before model exposure. The model receives a stable capsule containing:

- artifact kind and scope;
- rooted identity;
- decision-relevant fields;
- exact expansion handle;
- freshness/completeness status;
- bounded rendering.

The old capsule is never retrospectively replaced. Expansion appends new evidence.

## 4. Dependency invalidation

On project change `Delta`:

1. compute the dependency-complete affected cone;
2. mark affected derived objects Invalid or Unknown;
3. retain unaffected objects;
4. recompute from the change frontier;
5. stop propagation at exact-equality boundaries;
6. record runtime-discovered missing edges as counterexamples;
7. revoke leases and capabilities depending on invalid objects.

## 5. Q99 coordinates

### L2 logical reuse

For demanded object set `D`, current validity indicator `v_i`, and demand weights `w_i`:

```text
R_L2 = sum(w_i * v_i) / sum(w_i)
```

The denominator must include every demanded object in the declared scope, including misses and Unknown objects.

### L3 residency

For resident indicator `r_i`:

```text
R_L3 = sum(w_i * v_i * r_i) / sum(w_i * v_i)
```

This measures how much logically valid demanded mass is physically resident in the named tier.

### Provider prefix reuse

Use provider-reported or independently measured cached input tokens over the exact request. Do not infer provider hits from L2/L3 state.

### Model-visible context elimination

Compare exact model-visible tool arguments/results between same-model/same-harness baseline and treatment.

### Complete work

Include indexing, cache reads/writes, storage, transfer, validation, verification, failed speculation, restoration, and amortized preparation.

## 6. Draft 6 Causal Residency Budget Theorem

Let valid demanded objects be indexed by `i`, with positive demand weight `w_i`, size `s_i`, and resident decision `r_i in {0,1}`. Let capacity be `C`.

A physical tier can satisfy Q99 residency exactly when there exists a set `R` such that:

```text
sum_{i in R} s_i <= C
sum_{i in R} w_i >= 0.99 * sum_i w_i
```

Equivalently, the optimum of the 0-1 knapsack objective

```text
max sum(w_i * r_i) subject to sum(s_i * r_i) <= C
```

must be at least `0.99 * sum(w_i)`.

For uniform object sizes, the minimum number of resident objects is the shortest prefix of valid objects sorted by descending demand weight whose cumulative weight reaches 99%.

### Implementation consequence

The optimizer may be heuristic or approximate for large general instances. Authority is separate: a proposed residency plan receives a Q99 certificate only after a deterministic checker confirms its capacity and retained-weight inequalities.

## 7. Draft 6 Q99 Eviction Slack Theorem

Let current resident valid demand mass be `W_R`, total valid demand mass be `W`, and Q99 slack be:

```text
sigma = W_R - 0.99 * W
```

An eviction set `E` is guaranteed to preserve current Q99 when:

```text
weight(E intersect R) <= sigma
```

If the removed resident demand mass exceeds `sigma`, Q99 is not certified after eviction without compensating admissions or a new demand certificate.

### Implementation consequence

Every eviction transaction must calculate post-eviction demand mass before mutation. Recency alone cannot authorize an eviction that breaks a declared Q99 service contract.

## 8. Provider-Miss Bounded Amplification

Let `B` be the baseline model-visible project transcript that would be replayed after a provider miss, `C` the compact ZeroStack decision view, and `L` the model-visible overhead of resolving the continuation. Conditional on valid retained L2 state, the provider-miss model-visible burden is bounded by `C + L` rather than `B`.

The miss-insulation reduction is:

```text
1 - (C + L) / B
```

when `B > 0`. Backend L2/L3 work remains separately charged.

## 9. Windowed and restoration Q99

Report Q99 over declared sliding windows. After a change, record:

- invalidated demand mass;
- time and work to restore 99%;
- interval in which Q99 was mathematically impossible;
- equality-boundary cutoffs;
- provider misses during restoration;
- L3 fetch and recomputation costs.

Campaign averages may not hide post-change collapse.

## 10. Branch and cross-session reuse

Identical content and contract roots share one logical L2 object across branches, sessions, and harnesses. Branch identity is not semantic identity. Security and tenancy scopes remain part of authorization even when content deduplicates physically.

## 11. Eviction and retention policy inputs

A retention planner may use:

- observed task demand weight;
- predicted demand with confidence bounds;
- recomputation/hydration cost;
- object size;
- invalidation hazard;
- provider miss amplification avoided;
- capability/verifier criticality;
- storage tier and transfer cost.

Predictions may propose. Current validity, capacity, and Q99 checks authorize.

## 12. Required cache events

- object formed;
- object validated;
- logical hit/miss/Unknown;
- physical tier hit/miss;
- provider hit/miss observation;
- invalidation cause and cone;
- equality cutoff;
- admission/eviction proposal and certificate;
- materialization/fetch/recompute;
- Q99 window update;
- corruption, poisoning, or cross-scope rejection.
