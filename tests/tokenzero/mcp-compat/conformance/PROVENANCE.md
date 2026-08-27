# MCP JSON-RPC Conformance Provenance

No generated fixtures are used in this slice.

The table-driven cases in `../jsonrpc_conformance.rs` were derived by hand from:
- JSON-RPC 2.0 Specification: https://www.jsonrpc.org/specification
- MCP 2025-06-18 schema: https://modelcontextprotocol.io/specification/2025-06-18/schema
- MCP 2025-06-18 lifecycle: https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle
- MCP 2025-06-18 logging: https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/logging
- MCP 2025-06-18 prompts/list, resources/list, resources/read, tools/list, and tools/call result schemas: https://modelcontextprotocol.io/specification/2025-06-18/schema
- MCP 2025-06-18 transports: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
- MCP 2025-03-26 transports batch note: https://modelcontextprotocol.io/specification/2025-03-26/basic/transports
- MCP draft `server/discover` / 2026-07-28 release-candidate discovery contract: https://modelcontextprotocol.io/specification/draft/server/discover

Regeneration workflow:
1. Re-read the specification sources.
2. Update `../jsonrpc_conformance.rs` with one case per newly relevant
   envelope, lifecycle, discovery, method-contract, logging,
   pagination-param, or result-shape requirement.
3. Run `cargo test -p tokenzero-mcp --test jsonrpc_conformance`.
4. Review this directory and the test diff before committing.
