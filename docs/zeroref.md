# ZeroRef v1 fragment-bound policy

Status: normative for portable `(fz|gz|tz)://blob/...` refs.

| Dimension | Canonical policy | Boundary behavior |
| --- | --- | --- |
| byte start and end | strict | Any endpoint past the blob length returns `range_out_of_bounds`. An empty span at exactly the length is valid. |
| line start | strict | A start past the available line count returns `range_out_of_bounds`; the empty blob has zero lines. |
| line end | clamp | An end past EOF clamps to the last available line, provided the start is valid. |

Line fragments remain one-based and inclusive. Byte fragments remain zero-based and half-open. UTF-8 validation and full-object digest verification precede line selection.

The default is encoded by `zero_ref::CANONICAL_LINE_END_POLICY` and used by `ZeroRefV1::select`, `ZeroRefV1::verify_and_select`, and `select_fragment`. Explicit strict selection exists only for compatibility checks and exact-bound validation. Engines MUST NOT choose a different default.

## Capability advertisement

The policy is fixed by the ZeroRef v1 protocol version, not negotiable runtime behavior. Capability descriptors therefore advertise ZeroRef v1 support only and MUST NOT add a per-engine clamp-policy field. This document establishes the previously unspecified ZeroRef v1 default. After adoption, any policy change requires a ZeroRef protocol-version bump and new golden vectors, avoiding peers that claim the same version while selecting different bytes.
