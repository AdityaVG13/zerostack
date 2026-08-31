# Snap-to-file target reference grammar

FSZero and GraphZero use one target format for a file line window:

```text
<path>#L<start>-L<end>
```

- `path` is relative to the active workspace root.
- `start` and `end` are one-based and inclusive.
- `start >= 1` and `end >= start`.
- A read returns the bytes that cover that line window.
- An edit validates match uniqueness only inside the window, then applies the ordinary preimage, publication, receipt, and rollback rules.
- A missing preimage, an out-of-date line window, or changed bytes returns `stale_preimage`. Multiple matches return `conflict`.

`#L` is distinct from the byte fragment `#B<start>-<end>`, whose bounds are zero-based and half-open.

## Structural hits

A structural hit carries the target reference, match kind, enclosing symbol when known, and an exact bounded content window. A hit can therefore feed a guarded read or edit without reconstructing a path or line range from prose.

Typical match kinds are `literal`, `def`, `caller`, `import`, and `structural`. GraphZero owns syntax and relationship evidence. FSZero owns the byte range and preimage validation.

## Inline and recovery behavior

Small hit windows may be returned inline. Larger result sets return a bounded projection plus an exact content-addressed handle. The projection is not source authority. Reading the handle recovers the original result bytes.

## Safety properties

- Target references never escape the workspace root.
- Parsing rejects absolute paths, parent traversal, zero line numbers, reversed ranges, and integer overflow.
- A stale target never widens silently to the whole file.
- A structural hit does not bypass FSZero preimage validation.
