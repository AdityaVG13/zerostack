# ZeroStack documentation

ZeroStack is the source-only composition hub for the Zero family. The hub and the three engines live in one Cargo workspace under `crates/zerostack`, `crates/fszero`, `crates/graphzero`, and `crates/tokenzero`. The public model-facing product is the daemonless, in-process ZeroKernel host.

Contracts live in `contracts/` as a flat machine-readable surface. See `contracts/README.md` for the file inventory and change process.

## Hub

* [Architecture](architecture.md) - Authority boundaries and composition flow
* [Components](components.md) - Responsibilities of FSZero, GraphZero, and TokenZero
* [RACC](racc/RACC.md) - Recovery-aware output projection
* [Contributing](../CONTRIBUTING.md) - Workspace layout and contract rules

## FSZero

* [Overview](fszero/README.md) - Byte and filesystem authority
* [Architecture](fszero/architecture.md) - Engine layers, snapshots, and effects
* [Filesystem contract](fszero/filesystem-contract-v1.md) - Stable operation and error semantics
* [Installation](fszero/install.md) - Build the CLI and use FSZero with ZeroKernel
* [Development](fszero/development.md) - Contributor commands
* [Durability](fszero/durability.md) - Store publication and crash recovery invariants
* [Telemetry](fszero/telemetry.md) - Local usage accounting
* [Benchmark integrity](fszero/benchmark-integrity.md) - Evidence and publication requirements
* [Profiling](fszero/profiling.md) - Release-perf build and evidence workflow
* [Budgets](fszero/BUDGETS.md) - Performance budgets and variance envelope
* [Shared CAS](fszero/design/shared-cas.md) - Canonical shared content-addressed store
* [Target ref grammar](fszero/design/target-ref-grammar.md) - Snap-to-file target refs
* [Watch feed](fszero/design/watch-feed.md) - Change feed contract
* [ZeroRef v1 annex](fszero/design/zeroref-v1-annex.md) - ZeroRef reference annex

## GraphZero

* [Overview](graphzero/README.md) - Structure authority
* [Architecture](graphzero/architecture.md) - Crate layers and indexing
* [Installation](graphzero/install.md) - Build the CLI and compose through ZeroKernel
* [Benchmarks](graphzero/benchmarks.md) - Evidence and reproduction
* [Operation ABI](graphzero/operation_abi.md) - Typed request, result, coverage, and error boundary
* [API errors](graphzero/api_errors.md) - Stable public error classes
* [Threat model](graphzero/threat_model.md) - Trust and no-claim boundaries

## TokenZero

* [Overview](tokenzero/README.md) - Output authority
* [Installation](tokenzero/install.md) - Source builds and local setup
* [Development](tokenzero/development.md) - Build and verification
* [CodeMode](tokenzero/codemode.md) - ZeroKernel integration
* [MCP](tokenzero/mcp.md) - Noncanonical classic compatibility
* [Benchmarks](tokenzero/benchmarks.md) - Reproducible evidence
* [Pulse](tokenzero/pulse.md) - Local recovery-aware ledger
* [RACC](racc/RACC.md) - Projection and recovery

Typed Rust contracts live in `crates/zerostack/zero-abi`. Integration tests live in `tests/fszero`, `tests/graphzero`, and `tests/tokenzero` with shared helpers in `tests/support`. Conformance that every engine must pass lives in `crates/zerostack/zerostack-conformance`.
