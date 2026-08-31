# GraphZero documentation

GraphZero is ZeroStack's structure domain library. It owns syntax-aware search, symbols, relationships, freshness, coverage, impact, and structural evidence. ZeroKernel exposes this authority through `z.find`.

GraphZero is not a separate product and does not expose a model-facing tool catalog.

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | Domain layers, snapshots, query routing, freshness, and evidence |
| [Contributor setup](install.md) | Build GraphZero as part of ZeroStack |
| [Benchmarks](benchmarks.md) | Current claim-eligible evidence and reproduction |
| [Query contract](query-contract.md) | Typed domain requests, results, coverage, and errors |
| [API errors](api_errors.md) | Stable error classes |
| [Threat model](threat_model.md) | Trust and no-claim boundaries |

Normative contracts live under `contracts/`, including `SurfaceMatrix.toml` and the ZeroRef fixtures.
