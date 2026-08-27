# MCP JSON-RPC Conformance Coverage

Scope: JSON-RPC envelope behavior and MCP method-contract behavior in
`tokenzero_mcp::handle_jsonrpc`.

Specification sources:
- JSON-RPC 2.0 Specification: https://www.jsonrpc.org/specification
- MCP 2025-06-18 schema: https://modelcontextprotocol.io/specification/2025-06-18/schema
- MCP 2025-06-18 lifecycle: https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle
- MCP 2025-06-18 logging: https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/logging
- MCP 2025-06-18 transports: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
- MCP 2025-03-26 transports batch note: https://modelcontextprotocol.io/specification/2025-03-26/basic/transports
- MCP draft discovery / 2026-07-28 RC: https://modelcontextprotocol.io/specification/draft/server/discover

| Section | MUST Clauses | SHOULD Clauses | Tested | Passing | Divergent | Score |
|---------|-------------:|---------------:|-------:|--------:|----------:|-------|
| JSON-RPC 2.0 Parse | 1 | 0 | 1 | enforced by `jsonrpc_request_envelope_conformance_matrix` | 0 | 100% |
| JSON-RPC 2.0 Request Object | 6 | 1 | 7 | enforced by `jsonrpc_request_envelope_conformance_matrix` | 0 | 100% |
| JSON-RPC 2.0 Notifications | 2 | 0 | 2 | enforced by `jsonrpc_request_envelope_conformance_matrix` | 0 | 100% |
| JSON-RPC 2.0 Method Dispatch | 1 | 0 | 1 | enforced by `jsonrpc_request_envelope_conformance_matrix` | 0 | 100% |
| JSON-RPC 2.0 Batch | 4 | 0 | 4 | enforced by `jsonrpc_request_envelope_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Initialized Notification | 1 | 0 | 1 | enforced by `jsonrpc_request_envelope_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Initialize | 13 | 0 | 13 | enforced by `mcp_initialize_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Logging Set Level | 13 | 0 | 13 | enforced by `mcp_logging_set_level_conformance_matrix` | 0 | 100% |
| MCP Draft Discovery / 2026-07-28 RC | 9 | 1 | 10 | enforced by `mcp_server_discover_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Method Params | 11 | 0 | 11 | enforced by `mcp_method_params_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 List Result Shapes | 6 | 3 | 9 | enforced by `mcp_result_shape_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Resource Read Result Shape | 4 | 0 | 4 | enforced by `mcp_result_shape_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Tool Object Shape | 3 | 0 | 3 | enforced by `mcp_result_shape_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Tool Call Result Shape | 2 | 1 | 3 | enforced by `mcp_result_shape_conformance_matrix` | 0 | 100% |
| MCP 2025-06-18 Tool Result Errors | 0 | 1 | 1 | enforced by `mcp_tool_result_conformance_marks_tool_originated_errors` | 0 | 100% |

Out of scope for this slice:
- Per-tool input schema validation. Covered by unit tests around tool specs and call paths.
- MCP stdio `Content-Length` framing. Covered by `framed_stdio_parser_*`.
- Full session-level uniqueness of request IDs. The current dispatcher is stateless.
- Full lifecycle sequencing beyond stateless initialize and initialized validation.
- HTTP transport behavior. TokenZero's public MCP runtime target here is stdio.
- Pagination traversal semantics beyond validating `cursor` type. The current
  in-process catalog fits in one page and never emits `nextCursor`.
