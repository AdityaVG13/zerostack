# RACC durability parity for fz:// (fszero-g0i8)

Status: implementation evidence for transactional mint; stack-wide MMR deferred.

## Transactional mint (persist-then-ack)

Already implemented in `RecoveryStore::try_put_content_ref` / pack path:

1. Hash bytes → `fz://blob/<sha256>`.
2. Persist payload (`try_put_key`: pack `sync_all` before SQLite locator when packed).
3. Dual-write shared CAS when attached (fail-open for sharing only).
4. Append ref-index; **then** return Ok(ref).

Barrier class: `docs/durability.md` — WAL + `synchronous=FULL`; pack `sync_all` before locator.

## Fixture matrix

`tests/racc_durability_matrix.rs`:

| Class | Same-call expand | Reopen |
| --- | --- | --- |
| Inline blob | yes | yes |
| Packed blob (≥8KiB) | yes | yes |
| Named key | yes | yes |
| Memory put/get | session path | — |
| Digest identity | mint address = sha256 | — |
| Idempotent mint | same ref twice | — |

Also: `tests/crash_injection.rs` (`acked_packed_put_survives_reopen`, pack tear oracles).

## Shared MMR / inclusion proofs

TokenZero-style MMR transparency logs for **stack-wide** zero-loss audit are a
cross-engine program. FSZero alone provides per-store transactional mint and
digest-verified expand. Full inclusion proofs across engines are **out of
scope for this bead close** unless a shared mint-log crate lands later.

## Acceptance mapping

| Requirement | Status |
| --- | --- |
| Durability matrix fixtures + same-call expand | **green** (`racc_durability_matrix`) |
| Transactional mint | **green** (code + crash_injection + matrix) |
| Inclusion proofs (MMR) | **deferred** (stack-wide) |
