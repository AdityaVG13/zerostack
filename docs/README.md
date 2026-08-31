# ZeroStack documentation

ZeroStack is the program. ZeroKernel is its daemonless in-process execution host. FSZero, GraphZero, and TokenZero provide file, structure, and token domain logic from independent crate domains in the same Cargo workspace. They are not separate products and never import one another.

Machine-readable contracts live under `contracts/`.

## ZeroStack

* [Architecture](architecture.md) - Authority boundaries and composition flow
* [Components](components.md) - Responsibilities of FSZero, GraphZero, and TokenZero
* [RACC](racc/RACC.md) - Recovery-aware output projection
* [Contributing](../CONTRIBUTING.md) - Workspace layout and contract rules

## FSZero

* [Overview](fszero/README.md) - Byte and filesystem authority
* [Architecture](fszero/architecture.md) - Domain layers, snapshots, and effects
* [Filesystem contract](fszero/filesystem-contract.md) - Stable operation and error semantics
* [Contributor setup](fszero/install.md) - Build FSZero as part of ZeroStack
* [Development](fszero/development.md) - Focused contributor commands
* [Durability](fszero/durability.md) - Store publication and crash recovery
* [Telemetry boundary](fszero/telemetry.md) - Local domain measurements
* [Benchmark integrity](fszero/benchmark-integrity.md) - Evidence requirements
* [Profiling](fszero/profiling.md) - Release-performance evidence
* [Budgets](fszero/BUDGETS.md) - Performance budgets
* [Shared CAS](fszero/design/shared-cas.md) - Shared content-addressed storage
* [Target references](fszero/design/target-ref-grammar.md) - Snap-to-file targets
* [Watch feed](fszero/design/watch-feed.md) - Change feed contract
* [ZeroRef annex](fszero/design/zeroref-annex.md) - Portable content refs

## GraphZero

* [Overview](graphzero/README.md) - Structure authority
* [Architecture](graphzero/architecture.md) - Domain layers, indexing, and freshness
* [Contributor setup](graphzero/install.md) - Build GraphZero as part of ZeroStack
* [Benchmarks](graphzero/benchmarks.md) - Evidence and reproduction
* [Query contract](graphzero/query-contract.md) - Typed domain requests and results
* [API errors](graphzero/api_errors.md) - Stable error classes
* [Threat model](graphzero/threat_model.md) - Trust and no-claim boundaries

## TokenZero

* [Overview](tokenzero/README.md) - Output authority
* [Contributor setup](tokenzero/install.md) - Build TokenZero as part of ZeroStack
* [Development](tokenzero/development.md) - Focused verification
* [ZeroKernel integration](tokenzero/codemode.md) - TokenZero behind operation and response boundaries
* [MCP boundary](tokenzero/mcp.md) - Why MCP is not a TokenZero product
* [Benchmarks](tokenzero/benchmarks.md) - Reproducible output and latency evidence
* [Pulse](tokenzero/pulse.md) - ZeroStack-owned local accounting ledger
* [RACC](racc/RACC.md) - Projection and exact recovery

Typed cross-domain contracts live in `crates/zerostack/zero-abi`. Integration tests live under `tests/`, with reusable cross-domain fixtures in `tests/support/`. Shared conformance lives in `crates/zerostack/zerostack-conformance`.
