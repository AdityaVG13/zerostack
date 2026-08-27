# Snap-to-file target ref grammar (v1)

Bead: `fszero-snap-to-file-targets-99q7`. Defined once in `src/core/target_ref.rs`;
adopted by every FSZero discovery surface and by GraphZero (bead 5htnw).

## Target ref

    <path>#L<start>-L<end>

- `path` is relative to the session root, exactly as the surface indexes it.
- `start`/`end` are 1-based and inclusive; `start >= 1`, `end >= start`.
- `fs read` accepts this string VERBATIM. No re-derivation, no second discovery
  call. It returns the byte range covering that line window.
- `zero.fs.compound('mutate', {path: target, old, new})` accepts the same
  string verbatim. Match uniqueness is evaluated only inside the inclusive line
  window. The edit still uses the ordinary preimage revalidation, atomic write,
  certificate, durable history, and compensating undo path. A missing `old` or
  a target end beyond the current file line count is `stale_preimage`; multiple
  matches are `conflict`. Read windows retain their separate clamping behavior.
- `#L` is disjoint from the pre-existing `#B<start>-<end>` byte-range suffix and
  from ZeroRef blob fragments (`docs/design/zeroref-v1-annex.md`).

## Hit record

Every search / list / diagnostic hit is one record:

    HIT <path>#L<start>-L<end> kind=<match-kind> sym=<enclosing symbol>
    | <line-no>: <line text>
    | <line-no>: <line text>

- Header carries the target ref plus intent metadata: `kind` (match kind, e.g.
  `literal`) and `sym` (enclosing symbol, or `(file-scope)`).
- `|` lines are the content window, INLINED in the same response, byte-identical
  to the file, each prefixed by its 1-based line number.
- The window is the matched line plus `TARGET_CONTEXT_LINES` on each side.

### Match kinds

EVERY discovery route emits this record, so every route is one-call actionable:

| route | `kind=` | `sym=` |
| --- | --- | --- |
| grep / literal scan | `literal` | enclosing symbol inferred from the file |
| `asgrep:` literal rows | `asgrep` | enclosing symbol inferred from the file |
| `defs:` / `asgrep:` definitions | `def` | the defined symbol |
| `callers:` / `asgrep:` call edges | `caller` | the calling symbol |
| `imports` | `import` | the imported symbol |

Structural and AST-sgrep rows keep their legacy `DEF:` / `CALLER:` / `IMPORT:` /
`ASGREP:` line directly after the inlined window, so existing consumers that
match on those prefixes keep working.

## Inline guarantee

Payloads at or below `TARGET_INLINE_MAX_BYTES` (4096) are returned whole. Only
larger result sets fall back to a `ref=` expansion handle. A sub-4KB result is
never preview-only.

## Proof

`src/core/target_ref_proof.rs` simulates a fresh subagent: one substrate call,
then file + lines + content + intent and a scripted edit derived from that single
response. Transcript: `docs/design/one-call-target-proof.txt`.
