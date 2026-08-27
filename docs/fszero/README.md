# FSZero documentation

FSZero is the byte and filesystem authority for the Zero family. Public documentation is organized around exact reads, snapshots, guarded effects, recovery, and the standalone operator CLI.

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | Engine layers, byte authority, snapshots, effects, and recovery |
| [Installation](install.md) | Build the CLI and use FSZero with ZeroKernel |
| [Development](development.md) | Repository-relative contributor commands |
| [Durability](durability.md) | Store publication and crash-recovery invariants |
| [Filesystem contract](filesystem-contract-v1.md) | Stable operation and error semantics |
| [Telemetry](telemetry.md) | Default-off local usage accounting |
| [Benchmark integrity](benchmark-integrity.md) | Evidence and publication requirements |
| [Profiling](profiling.md) | Release-perf build and evidence workflow |
| [Budgets](BUDGETS.md) | Performance budgets and variance envelope |
| [Shared CAS](design/shared-cas.md) | Canonical shared content-addressed store |
| [Target ref grammar](design/target-ref-grammar.md) | Snap-to-file target refs |
| [Watch feed](design/watch-feed.md) | Change feed contract |
| [ZeroRef v1 annex](design/zeroref-v1-annex.md) | ZeroRef reference annex |

Contracts for FSZero live in `contracts/` at the repository root. Conformance lives in `crates/zerostack/zerostack-conformance`.
