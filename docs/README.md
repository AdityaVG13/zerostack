# ZeroStack documentation

ZeroStack is the source-only composition hub for the Zero family. The public model-facing product is the daemonless, in-process ZeroKernel host.

| Document | Purpose |
| --- | --- |
| [ZeroKernel specification](zero-kernel.md) | Canonical six-operation surface, lifecycle, effects, state, and response contract |
| [ZeroKernel cells](zero-kernel-cells.md) | Writing bounded JavaScript and TypeScript cells against `z` |
| [Architecture](architecture.md) | Authority boundaries and composition flow |
| [Components](components.md) | Responsibilities of FSZero, GraphZero, and TokenZero |
| [RACC](racc/RACC.md) | Recovery-aware output projection and exact expansion |

Current benchmark evidence lives under [`benchmarks/`](../benchmarks/README.md). Typed Rust contracts live in the workspace crates. Historical execution catalogs are preserved by Git history rather than documented as supported alternatives.
