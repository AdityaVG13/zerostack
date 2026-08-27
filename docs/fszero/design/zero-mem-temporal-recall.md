# Zero-Mem temporal recall (fszero-ute2)

Maps [Zero-Mem (arXiv:2607.29377)](https://arxiv.org/abs/2607.29377) "zero-token
durable memory" onto FSZero's **journal-native** surfaces.

## Authority boundary

| Zero-Mem concept | FSZero owner surface | Non-claim |
|---|---|---|
| Durable store of agent facts | `mem://` path-keyed payloads (`src/core/memory.rs`) + recovery SQLite | Not a vector DB; not semantic search quality |
| Temporal append log | Mutation journal (`recovery/mutation_log`, `journal_delta`) | Not external process/DB restore |
| Recall without model calls | `temporal_recall` pure journal scan + exact byte rehydrate | Never invents missing history; miss is typed |
| Episode / world state | Worlds + journaled publication (existing) | Hub admits quality; FSZero supplies exact bytes |

## Zero-token rule

Recall operators take **only** store/journal inputs (path, seq range, time
range, content digest). They return exact journal rows and optional byte
snapshots. They must not:

- call a model or embedding service
- silently drop or synthesize mutations
- claim Q99 savings (use labeled Q99-State/Input/Total denominators only when
  receipts exist)

## API (shipped)

`fs_zero::temporal_recall::{TemporalQuery, TemporalHit, recall_mutations}`

- Input: after_seq, path prefix, limit
- Output: ordered exact mutation rows (seq, op, path, digests)
- Implementation: `RecoveryStore::query_mutations*` — no model path

## Acceptance

1. Design maps paper → production modules without peer-engine imports.
2. Unit/integration test recalls put/delete sequence with zero model calls.
3. Stale/missing path returns empty or typed miss — never fabricated history.
