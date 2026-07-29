# Fresh-work accounting vector v1

Status: active. Implementation: `crates/zero-ledger/src/fresh_work.rs`.
Conformance suite: `crates/zero-ledger/tests/fresh_work_conformance.rs`.

## Why

Token savings alone cannot show that redundant work is disappearing. A cheap
action that re-derives information the session already paid for still costs. The
pay-once causal information law predicts cost
`O(new-instructions + changed-objects + causal-cut-width + log repo-size)`; to
falsify it we need to measure *causally novel* work per action, not the total.

## The vector

Every action's declared model-visible input is decomposed across exactly four
components. The set is exhaustive and disjoint: each token sits in exactly one.

| Component | Field | Meaning |
| --- | --- | --- |
| `FreshWork` | `fresh_work_tokens` | Causally novel work: new instructions and changed objects the session has never paid for. |
| `Replayed` | `replayed_tokens` | Served from prior information: cache hits, replayed evidence, re-exposed spans. |
| `Recovery` | `recovery_tokens` | Recovering or re-expanding already-paid information: verification, retries, re-expansion. |
| `Overhead` | `overhead_tokens` | Structural cost carrying no repository information: schema, protocol framing, harness scaffolding. |

`total_tokens` is **derived, never caller-supplied**. `FreshWorkVector::new`
computes it with checked addition, and the deserializer recomputes it and
rejects any wire value that disagrees
(`LedgerError::FreshWorkTotalMismatch`). A vector therefore cannot understate a
component or inflate the total to flatter the metric.

The all-zero vector means *undeclared*: the action did not report a
decomposition. It is the `Default`, so existing `TokenCharge` constructions
using `..TokenCharge::default()` keep compiling and keep their previous
semantics.

## eta_action

```
eta_action = fresh_work_tokens / total_tokens
```

Reported as `RetainedFractionPpm` (floored parts per million), so there are no
floats and no percentage strings. Because the components sum to the total, the
result is always inside `[0, 1_000_000]` ppm, i.e. `[0, 1]`. An undeclared
vector has no denominator and yields `None`.

Target: `eta_action -> 0` as transformations grow but stay structurally
describable — the action emits only the irreducible novel delta.

## Aggregation

`ActionFreshWork` pairs a nonempty action id with one vector.
`SessionFreshWork` folds action vectors component-wise with checked addition and
exposes `eta_session_ppm()`: the same ratio over the summed vector, i.e. the
novelty fraction `J_fresh / J_raw` of everything the session paid for.
Component-wise addition is associative and commutative, so the session eta does
not depend on action order.

## Ledger integration

`TokenCharge::fresh_work` carries the per-call vector; `TokenLedger.fresh_work`
is the cumulative aggregate and `TokenLedger.fresh_work_actions` counts the
charges that declared one. Both ledger fields are `#[serde(default)]`, so v1
ledger JSON still deserializes.

`ResourceGauge::charge` validates the vector before mutating anything: a
declared vector whose component sum differs from `input_tokens` is rejected with
`FreshWorkTotalMismatch` and the gauge's history is untouched. The vector is
`Copy` and accumulation is integer adds, so `charge()` still allocates nothing
(`tests/allocation.rs`).

## Conformance requirements

An implementation of this contract must satisfy:

1. **Component sum.** `component_sum() == total_tokens()` for every reachable
   vector, including aggregates.
2. **Eta bounds.** `eta_action_ppm()` is `None` iff `total_tokens() == 0`,
   otherwise `<= PPM_ONE`.
3. **Monotonicity.** Shifting tokens from any other component into
   `fresh_work` at fixed total never lowers eta.
4. **Serde round trip.** Encode/decode is the identity, and a forged
   `total_tokens` fails closed.
5. **Aggregation.** Session aggregate equals the component-wise sum and is
   order-independent.
