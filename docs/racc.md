# RACC and typed refs

**Recovery-aware context compression (RACC)** reduces visible context without discarding the underlying evidence.

## The model

1. A tool produces a potentially large result.
2. The result is stored in a content-addressed or otherwise recoverable backing store.
3. The model receives a short typed ref plus a bounded preview.
4. Later work expands only the needed lines, symbols, or raw bytes.

ZeroStack uses namespace-specific refs:

| Ref | Producer | Typical content |
| --- | --- | --- |
| `tz://` | TokenZero | Compacted logs, search output, shell output |
| `fz://` | FSZero | File reads, searches, plans, mutation receipts |
| `gz://` | GraphZero | Graph snapshots, impact paths, orientation results |

## Why recoverability matters

Ordinary truncation saves tokens by throwing data away. Summaries save tokens but may omit details. RACC keeps the full result recoverable, so an agent can verify a claim later without rerunning the original operation.

This enables:

- small previews for orientation;
- exact bounded expansion for verification;
- ref passing between engines and plan steps;
- deduplication of repeated results;
- audit trails that point back to source evidence.

## Discipline

Refs are useful only when consumers preserve their type and recovery path. Integrations should avoid copying full payloads into context, should expand the smallest sufficient range, and should surface an explicit error when a ref is unavailable or expired.
