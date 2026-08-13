# Benchmarks

ZeroStack is not a published product. These are local development measurements.

## Aggregate CodeMode

Compare in-process `zsx-core` at three granularities:

- one aggregate model-visible cell
- one standalone CodeMode cell per engine
- one model-visible cell per logical operation

Corpus: mixed three-engine work, exact refs, shell, mutation, cancellation, failure.

Previously this lived as a Node harness under `bench/aggregate-codemode/`. That
harness is parked with the rest of the install/CLI clutter. Reintroduce a
single runner here only when we need a new number, and keep the runner out of
this folder -- this file is the catalog.

## Native warm read

Warm `zsx` / CodeMode read path. Same rule: one catalog entry, no extra schema
or validate scripts in this folder.

## Crate microbenchmarks

| Crate | Bench | Command |
|---|---|---|
| `zero-cert` | `verify` | `cargo bench -p zero-cert --bench verify` |
| `zero-gate` | `decide` | `cargo bench -p zero-gate --bench decide` |
| `zero-ledger` | `charge` | `cargo bench -p zero-ledger --bench charge` |
| `zero-ref` | `parse` | `cargo bench -p zero-ref --bench parse` |

Sources: `tests/benches/<crate>/`.

## Rule

Do not ratchet a number from a single run. Keep-gate honesty is
`zerostack-keep-gate-honesty-063e`.
