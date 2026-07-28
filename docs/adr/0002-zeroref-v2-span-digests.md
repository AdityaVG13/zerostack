# ADR 0002: Keep v2 span digests in structured SpanRef

- Status: Accepted
- Date: 2026-07-28

## Context

A ZeroRef v1 textual reference identifies a complete object. Verifying a selected span from that reference therefore requires access to the complete object digest domain. ZeroRef v2 consumers, including evidence certificates, need to verify an already-resident span without fetching or hashing the complete object.

Adding a span digest to the textual grammar, for example with a `;sd=` suffix, would create a second wire representation and force every parser, formatter, cache key, fixture, and engine boundary to understand a feature that only structured consumers currently require.

## Decision

For v2, `span_digest` exists only in the structured `zero_ref::SpanRef`. A `SpanRef` carries both the object digest and the span digest, together with the selection needed to identify the span.

The ZeroRef v1 textual grammar is unchanged. All existing textual references retain their exact syntax and meaning. There is no `;sd=` syntax, and structured span digests are not serialized by appending data to a v1 reference.

`span_digest` is SHA-256 over the exact selected span bytes produced by the existing canonical selection path, `select_fragment`, under `CANONICAL_LINE_END_POLICY`. Selection boundaries are interpreted by that canonical path. Line endings are handled only as that policy specifies; implementations must not add platform-native conversion, Unicode normalization, whitespace folding, or any other transformation before hashing. The bytes returned by canonical selection are the digest domain, including their canonical line endings and exact boundary behavior.

At object-write time, construction uses one input buffer and one traversal of the object bytes. The implementation updates the object hash while consuming that buffer and updates each applicable span hash from the corresponding selected byte region during the same pass. It must not require a second object read or construct a second full-object buffer. A selected span may be represented as a view or bounded span buffer as needed by the API.

## Rationale

Binding the digest to the selected bytes lets a verifier hash a resident span and compare a fixed 32-byte value. This avoids a full-object fetch and full-object hash, reduces verification work from object-sized input to span-sized input, and supports stable structured cache keys.

Keeping the digest structured also limits the security boundary. One canonical selector and line-end policy define the hashed bytes, so alternate textual spellings cannot create parser differentials, ambiguous cache identities, or downgrade behavior between v1-aware and v2-aware components. The object digest remains available to bind the span to its source object; the span digest does not replace it.

## Compatibility

This decision is additive at the type/API layer and non-breaking at the textual layer. Existing v1 references, parsers, formatters, golden vectors, stored values, and cache keys remain byte-for-byte valid. Components that do not consume `SpanRef` need no change. Structured v2 consumers must carry and verify both digests according to their protocol.

## Alternatives considered

### Add a span digest to the textual grammar now

Rejected. A form such as `#B<start>-<end>;sd=<64hex>` expands the public grammar before a wire-format requirement exists. It increases parser and canonicalization surface, risks inconsistent treatment by older consumers, and can split identity between structured and textual representations.

### Verify spans only through the object digest

Rejected. This preserves v1 semantics but requires the complete object for verification, defeating resident-span verification and imposing object-sized I/O and hashing.

### Make the span digest replace the object digest

Rejected. A span digest proves the selected bytes but does not bind them to the intended source object. Both identities are required for structured evidence.

## Consequences

Structured producers must compute `span_digest` using the canonical selector and expose it with the object digest. Structured verifiers can authenticate resident spans without retrieving the complete object. Textual ZeroRef behavior and all existing references remain unchanged.

A later additive textual grammar may be considered only when an actual interoperable wire use case requires a self-contained textual span digest. That proposal must define unambiguous version negotiation, canonical serialization, strict parsing and rejection rules, digest algorithm agility, downgrade behavior for v1-only consumers, cache-key semantics, security test vectors, and byte-for-byte preservation of every valid v1 reference. It must be introduced as an explicit additive version, not by retroactively changing v1.
