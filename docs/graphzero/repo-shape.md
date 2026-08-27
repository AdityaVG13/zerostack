# GraphZero repository shape

GraphZero is a Rust 2024 workspace organized by authority rather than model-facing transport.

| Area | Responsibility |
| --- | --- |
| `graphzero-types` | Stable domain types and errors |
| `graphzero-core` | Shared graph primitives |
| `graphzero-store` | Snapshots, indexes, refs, and durable graph state |
| `graphzero-engine` | Typed query dispatch |
| `graphzero-kernel` | Adapter consumed by ZeroKernel |
| `graphzero-extract` | Language and syntax extraction |
| `graphzero-cli` | Standalone operator commands |
| specialized crates | Coverage, packaging, reservations, SCIP, semantic retrieval, and explanations |

The workspace root owns shared package metadata and dependencies. Integration tests live under `tests/` and are declared by the owning crate. GraphZero depends on ZeroStack contract crates but does not import FSZero or TokenZero.

Historical `graphzero-query`, engine-local CodeMode, and per-engine MCP packages are not part of the current workspace.
