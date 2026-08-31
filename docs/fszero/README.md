# FSZero documentation

FSZero is ZeroStack's byte and filesystem domain library. It owns exact reads, snapshots, guarded effects, restoration, and filesystem durability. ZeroKernel exposes this authority through `z.read`, `z.edit`, and `z.apply`.

FSZero is not a separate product and does not expose a model-facing tool catalog.

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | Domain layers, byte authority, snapshots, effects, and recovery |
| [Contributor setup](install.md) | Build FSZero as part of ZeroStack |
| [Development](development.md) | Repository-relative contributor commands |
| [Durability](durability.md) | Store publication and crash-recovery invariants |
| [Filesystem contract](filesystem-contract.md) | Stable domain and error semantics |
| [Telemetry boundary](telemetry.md) | Local filesystem measurements |
| [Benchmark integrity](benchmark-integrity.md) | Evidence and publication requirements |
| [Profiling](profiling.md) | Release-performance evidence workflow |
| [Budgets](BUDGETS.md) | Performance budgets and variance envelope |
| [Shared CAS](design/shared-cas.md) | Shared content-addressed storage |
| [Target references](design/target-ref-grammar.md) | Snap-to-file targets |
| [ZeroRef annex](design/zeroref-annex.md) | Portable content refs |

Shared machine-readable contracts live under the repository-root `contracts/` directory. FS behavior contracts live in the typed adapter and focused root tests. Shared conformance lives in `crates/zerostack/zerostack-conformance`.
