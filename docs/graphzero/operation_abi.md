# GraphZero operation ABI

GraphZero exposes typed Rust contracts to ZeroKernel. The model-facing entry is `z.find`; engine names and transport catalogs remain host details.

## Request

A request identifies query text or a structured relationship request, mode, workspace-relative scope, language when syntax requires it, result limit and output budget, freshness policy, and source and sink for call-path queries. Unsupported combinations fail typed. A malformed pattern is never a clean no-match.

## Result

A result carries ranked evidence, resolved symbol identity when applicable, snapshot and index digest, freshness, tier and scope coverage, truncation and continuation, exact refs, and an absence classification when no items are returned.

## Authority

GraphZero may publish graph-store updates needed for indexing and freshness. It must not mutate workspace source files. Exact bytes and file effects belong to FSZero. Output projection belongs to TokenZero.

## Cancellation

Requests run under the ZeroKernel frame cancellation token. Cancellation must not publish a partial snapshot as current. Cold work may return a retryable outcome with recovery guidance.
