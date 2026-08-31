# ZeroStack documentation

ZeroStack is one product. ZeroKernel is its daemonless in-process host. Files, structure, and tokens are domain libraries in the same Cargo workspace. They are not separate products and never import one another.

Machine-readable contracts live under `contracts/`.

## Product

* [Architecture](architecture.md) — Authority boundaries and composition flow
* [Components](components.md) — Hub crates and domain responsibilities
* [Build and verify](build.md) — Clone, build `zero-kernel`, and run focused checks
* [RACC](racc/RACC.md) — Recovery-aware output projection
* [Contributing](../CONTRIBUTING.md) — Workspace layout and contract rules

## Files

Byte and filesystem authority behind `z.read`, `z.edit`, and `z.apply` (`crates/fszero/`).

* [Architecture](files/architecture.md) — Domain layers, snapshots, and effects
* [Filesystem contract](files/filesystem-contract.md) — Stable operation and error semantics
* [Development](files/development.md) — Focused contributor commands
* [Durability](files/durability.md) — Store publication and crash recovery
* [Telemetry boundary](files/telemetry.md) — Local domain measurements
* [Benchmark integrity](files/benchmark-integrity.md) — Evidence requirements
* [Profiling](files/profiling.md) — Release-performance evidence
* [Budgets](files/BUDGETS.md) — Performance budgets
* [Shared CAS](files/design/shared-cas.md) — Shared content-addressed storage
* [Target references](files/design/target-ref-grammar.md) — Snap-to-file targets
* [Watch feed](files/design/watch-feed.md) — Change feed contract
* [ZeroRef annex](files/design/zeroref-annex.md) — Portable content refs

## Structure

Repository structure authority behind `z.find` (`crates/graphzero/`).

* [Architecture](structure/architecture.md) — Domain layers, indexing, and freshness
* [Query contract](structure/query-contract.md) — Typed domain requests and results
* [API errors](structure/api_errors.md) — Stable error classes
* [Threat model](structure/threat_model.md) — Trust and no-claim boundaries
* [Benchmarks](structure/benchmarks.md) — Evidence and reproduction

## Tokens

Output measurement, projection, and accounting honesty (`crates/tokenzero/`).

* [Pulse](tokens/pulse.md) — ZeroStack-owned local accounting ledger
* [Benchmarks](tokens/benchmarks.md) — Reproducible output and latency evidence
* [Development](tokens/development.md) — Focused verification
* [RACC](racc/RACC.md) — Projection and exact recovery

Typed cross-domain contracts live in `crates/zerostack/zero-abi`. Integration tests live under `tests/`, with reusable cross-domain fixtures in `tests/support/`. Shared conformance lives in `crates/zerostack/zerostack-conformance`.
