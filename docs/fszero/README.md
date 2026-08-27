# FSZero documentation

FSZero is the byte and filesystem authority for the Zero family. Public documentation is organized around exact reads, snapshots, guarded effects, recovery, and the standalone operator CLI.

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | Engine layers, byte authority, snapshots, effects, and recovery |
| [Installation](install.md) | Build the CLI and use FSZero with ZeroKernel |
| [Development](development.md) | Repository-relative contributor commands |
| [Durability](durability.md) | Store publication and crash-recovery invariants |
| [Filesystem contract](filesystem-contract-v1.md) | Stable operation and error semantics |
| [Memory](memory.md) | Durable path-shaped agent memory |
| [Telemetry](telemetry.md) | Default-off local usage accounting |
| [Benchmark integrity](benchmark-integrity.md) | Evidence and publication requirements |

Detailed design notes record implementation history. When an implemented invariant becomes stable, the canonical documents above should absorb it so temporary campaign notes can be removed.
