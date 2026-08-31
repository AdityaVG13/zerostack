# TokenZero documentation

TokenZero is ZeroStack's token and output domain library. It owns measurement, bounded projection, exact recovery, and accounting honesty at ZeroKernel operation and response boundaries.

TokenZero is not a separate product. It does not own MCP, process lifecycle, filesystem bytes, or graph structure.

| Document | Purpose |
| --- | --- |
| [RACC](../racc/RACC.md) | Projection, protected anchors, exact recovery, and accounting |
| [Contributor setup](install.md) | Build TokenZero as part of ZeroStack |
| [Pulse](pulse.md) | ZeroStack-owned local accounting ledger |
| [Benchmarks](benchmarks.md) | Reproducible output and latency evidence |
| [Development](development.md) | Workspace layout and focused verification |

Normative machine-readable contracts live under the repository-root `contracts/` directory. Shared conformance lives in `crates/zerostack/zerostack-conformance`.
