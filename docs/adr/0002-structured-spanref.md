# ADR 0002: Structured SpanRef identity

- Status: Accepted
- Date: 2026-07-28

## Decision

Span digests live only in the serde-compatible structured `zero_ref::SpanRef` type. ZeroRef v1 textual parsing and display remain byte-for-byte behavior compatible. In particular, this decision adds no `;sd=` fragment syntax.

`SpanRef` carries the complete object identity and digest, selected byte start and length, and SHA-256 of the exact selected bytes. `zero-ref` owns `Digest`, `ObjectId`, and `SpanRef`; certificate consumers re-export those canonical types rather than defining competitors.

Construction accepts one borrowed complete-object buffer and a `ZeroFragment` in one call. It delegates selection to `select_fragment` under `CANONICAL_LINE_END_POLICY`, then computes object and selected-byte digests from that resident borrow. It requires no second I/O or read API. Span-only verification needs only the selected payload; full object identity, range, and span verification remains available. Arithmetic and bounds failures are typed and never panic.

## Consequences

The structured wire can independently authenticate resident span payloads while preserving every v1 ref and parser vector. A later additive textual grammar is justified only if a wire ref must carry a span digest across a boundary that cannot carry the structured type. Such work requires a separate versioned ADR and cannot reinterpret v1 text.
