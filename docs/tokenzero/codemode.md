# TokenZero behind ZeroKernel

TokenZero does not own a model-facing JavaScript planner. ZeroStack owns the host, fresh frame, orchestration, process lifecycle, transactions, cancellation, and terminal response.

| Concern | Owner |
| --- | --- |
| Tokenizer identity and exact counts | TokenZero |
| Classification, projection, compression, and protected anchors | TokenZero |
| Exact recovery and recovery-aware accounting | TokenZero |
| Six-operation cell API | ZeroStack |
| File bytes and effects | FSZero |
| Structural evidence and freshness | GraphZero |

TokenZero runs automatically at `z.read`, `z.find`, `z.run`, and terminal response boundaries. Small values pass through. Larger values may return a bounded capsule, protected anchors, exact handles, and accounting.

Expansion returns original bytes. Tokens recovered later are added back to task cost, so an initial compression percentage is not misreported as net savings.

Any retained binary name containing `codemode` is rollout compatibility for a planner-free engine transport, not an engine-local execution surface. Clients unable to embed ZeroKernel may use classic MCP as documented in [mcp.md](mcp.md).
