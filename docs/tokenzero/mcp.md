# TokenZero classic MCP compatibility

Classic MCP exists for clients that require direct TokenZero tools and cannot embed ZeroKernel.

## Start

```bash
tokenzero mcp-server --mode=mcp
```

The server uses process-owned stdio and opens no network listener.

## Capability groups

| Group | Purpose |
| --- | --- |
| Read and find | Return bounded output plus exact refs |
| Expand and recall | Recover payloads, ranges, anchors, and hits |
| Tree and glob | Return bounded repository shape and paths |
| Shell ingestion | Measure and project captured output |
| Cache and memory | Inspect recovery state |
| Install and doctor | Plan integration and report health |

Exact names and schemas come from the running server catalog. Documentation does not duplicate a generated catalog that can drift.

Every request has a finite budget and resolves as completed, cancelled, failed, or panicked. A `tz://` ref is an identifier, not a public endpoint.

Use classic MCP for direct TokenZero compatibility. Use ZeroKernel for multi-engine workflows. Registering both creates overlapping routes and obscures accounting.
