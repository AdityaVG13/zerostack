# GraphZero documentation

GraphZero is the structure authority for the Zero family. Canonical documents describe syntax-aware search, graph relationships, freshness, coverage, impact, and evidence recovery.

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | Crate layers, snapshots, query routing, freshness, and evidence |
| [Installation](install.md) | Build the standalone CLI and compose through ZeroKernel |
| [Benchmarks](benchmarks.md) | Current claim-eligible evidence and reproduction |
| [Operation ABI](operation_abi.md) | Typed request, result, coverage, and error boundary |
| [API errors](api_errors.md) | Stable public error classes |
| [Threat model](threat_model.md) | Trust and no-claim boundaries |

Contracts for GraphZero live in `contracts/` at the repository root, including `SurfaceMatrix.toml` and the ZeroRef fixtures. Release gates enforce the pinned digest in `contracts/approved_operation_abi_digest.txt`.

Historical implementation campaigns remain in Git history. Public docs do not direct users to removed query packages, engine-local planners, or retired MCP catalogs.
