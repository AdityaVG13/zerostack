# FSZero durability and failure contract

Persistent FSZero state fails closed on corruption or unavailable storage. An in-memory
session is allowed only when the caller explicitly sets `FSZERO_ALLOW_EPHEMERAL=1`.

## Storage guarantees

- Content-addressed blobs are SHA-256 verified before bytes are returned.
- SQLite uses WAL journaling with `synchronous=FULL` for durable commits.
- Packed payload bytes reach a full file barrier before their locator transaction commits.
- Canonical CAS objects use a fully synced temporary file, atomic rename, and directory sync.
- Journal pages reject missing, reordered, or truncated sequence numbers.
- Rebuildable indexes may be discarded after damage. They are not durability roots.

The publication order is data, durability barrier, then locator visibility. A crash can
leave unreferenced bytes, but it must not expose a locator for a short or unverified payload.

## Failure behavior

Digest mismatches, torn pack extents, malformed ref-index rows, and store-open failures remain
distinct errors. FSZero never serves bytes that fail their digest. Recovery stages state in
memory and publishes it only after the complete page or object validates.

Restore an unrecoverable blob from the workspace or another verified store. Rebuild derived
indexes after cache corruption. Do not reuse a store whose primary database cannot open.

## Diagnostics

- `FSZERO_CAS_PHASES=1` records CAS write, sync, rename, directory-sync, read, and verify phases.
- `FSZERO_DURABLE_PUT_PHASES=1` records pack-sync and commit phases.
- `FSZERO_ATOMIC_WRITE_PHASES=1` records preparation, write, sync, rename, and directory-sync phases.

All three diagnostics are off by default. Their values are measurements, not durability proof.
