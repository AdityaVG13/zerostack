# ZeroRef annex

ZeroRef is the portable content-reference contract shared by ZeroStack, FSZero, GraphZero, and TokenZero. The canonical implementation is `crates/zerostack/zero-ref`. Golden vectors live in `contracts/zeroref-fixtures.json`.

## Scope

The portable grammar covers blob refs only:

```text
z://blob/<sha256>
z://blob/<sha256>#B<start>-<end>
z://blob/<sha256>#L<start>-<end>
```

Engine-internal sequence, node, query, session, and compact references are not portable ZeroRefs. Retired engine-tagged blob schemes fail closed at this boundary.

## Identity

- The hash is the full lowercase 64-hex SHA-256 of the complete unfragmented bytes.
- Short prefixes, uppercase hex, non-hex characters, and extra path segments are malformed.
- A fragment selects from a verified object. It never changes object identity.
- Parsing and re-emitting a canonical ref is byte-identical.

## Fragment semantics

Byte fragments use `#B<start>-<end>`. Bounds are zero-based and half-open. Byte selection works on arbitrary bytes.

Line fragments use `#L<start>-<end>`. Bounds are one-based and inclusive. Line selection requires valid UTF-8. A final unterminated line still counts as one line.

Strict selection never clamps. Reversed bounds, integer overflow, and bounds beyond the verified object return a typed error.

## Verification and resolution

A resolver must:

1. Parse the reference.
2. Resolve the full object from an allowed store.
3. Hash the full object and compare it with the reference identity.
4. Apply the fragment only after the digest matches.

Scheme or string rewriting never proves that bytes exist. Missing bytes, denied storage policy, I/O failure, and digest mismatch remain distinct errors.

## Error classes

| Class | Meaning |
| --- | --- |
| `malformed` | The input does not match the portable grammar. |
| `unsupported` | The input is recognizable but outside the portable blob-ref scope. |
| `range_out_of_bounds` | A strict fragment exceeds the verified object. |
| `not_utf8` | A line fragment targets non-UTF-8 bytes. |
| `missing` | No allowed store contains the object. |
| `io` | Store access failed. |
| `digest_mismatch` | Resolved bytes do not match the content identity. |
| `policy_denied` | Storage policy forbids resolution. |
| `incompatible_version` | A peer advertises an incompatible contract. |
| `legacy_ambiguity` | A legacy prefix resolves to zero or multiple objects. |

## Authority boundary

ZeroKernel returns portable `z://blob/...` handles. FSZero owns exact bytes and guarded selection. GraphZero may attach structural evidence to a handle. TokenZero may project the visible result. Neither graph metadata nor a compact projection can replace the verified bytes.
