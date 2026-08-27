# Canonical shared CAS (fszero-zjt)

Immutable blobs at `<store root>/blobs/sha256/<xx>/<64-lowercase-hex>`;
object bytes are the raw content only (identity = sha256 of complete bytes,
ZeroRef v1). Implementation: `src/core/cas.rs` (`CasStore`), 15-test
contract in `tests/cas.rs`.

## Ownership and activation

- `CasStore` is a plain handle; NO process-global state.
- The store root is the effective ZeroStack root (`zerostack_store.rs`
  precedence: `<repo>/.zerostack` when it exists, else `ZEROSTACK_STORE_ROOT`
  and only when `FSZERO_SHARED_STORE`/`ZEROSTACK_SHARED_STORE` opts in). The
  local marker wins because it is an explicit per-repo declaration, while the
  env is ambient; this matches TokenZero and GraphZero exactly (zerostack-pi1).
- Presence of the `blobs/` directory IS the opt-in: FSZero never creates it
  implicitly; `CasStore::detect` attaches only when it exists. Default
  project behavior is unchanged (project isolation preserved; a shared
  machine/team root is an explicit env decision).

## Concurrency and write protocol

hash-first → same-directory unique temp (`<hash>.<pid>.<seq>.tmp`,
`O_EXCL`) → flush + fsync → atomic rename-publish → shard-dir fsync (unix).
Existing destination: verify digest+length of the EXISTING file — identical
= idempotent success, mismatch = typed `Corrupt`, never overwritten.
Concurrent identical writers converge (verify-then-accept covers rename
races). Stale temps (>15 min) are swept by the next `put` in the shard.

## Tier precedence and fallback (reads)

For strict `(fz|gz|tz)://blob/<hash>` expansion (`expand_zeroref`):

1. **canonical CAS** (when attached) — verified `get`; a CORRUPT canonical
   object is terminal (`ref_unrecoverable … cas_corrupt`, loud via
   `integrity_report()`), never a silent fallback; a clean miss falls
   through.
2. legacy sqlite/pack store (digest-verified, fszero-ku8).
3. per-user ref-index (cross-store recovery, digest-verified).

Misses name the active tier list. Mints (`try_put_content_ref`) dual-write
into the CAS fail-open (legacy store already holds the bytes; a CAS write
failure only costs sharing).

## Integrity and platform guarantees

- Reads verify the full digest before serving; length is additionally
  checked when verifying an existing destination on `put`.
- Atomicity relies on same-filesystem `rename(2)`; temp + object share the
  shard directory by construction. fsync of file and shard dir on unix;
  rename-over-existing races degrade to verify-then-accept elsewhere.
- Checkout dedup: two checkouts configured with the same explicit store
  root mint into one object set — second checkout's puts are verified
  no-ops (benchmarks/cas_dedup.py, fszero-qhz is the ongoing acceptance).

Non-goals: paths/provenance/mutable indexes inside object bytes; implicit
shared roots; cross-engine blob SERVING beyond what the store already holds
(the ZeroRef v1 same-store limitation still applies to gz/tz claims).

## Legacy inventory (fszero-c6q.3)

Every production payload location and emitted ref shape that predates the
canonical CAS, grounded in `src/core/recovery.rs`:

| location | row/format | ground truth |
| :-- | :-- | :-- |
| `payloads` table, inline rows | first byte TAG `0x01` + raw content (`PAYLOAD_TAG_INLINE`) | `encode_payload_row` / `decode_payload_row` |
| `payloads` table, packed rows | 13-byte TAG `0x00` locator `(offset u64 le, len u32 le)` into the pack sidecar; payloads >= 4096 bytes (`PACK_MIN_BYTES`) | `encode_packed_locator` / `decode_packed_locator`; sidecar `store.sqlite3.pack[.gN]`, generation in meta key `pack_gen` (`pack_gen_path`, `compact_pack`) |
| `payloads` table, legacy UNTAGGED rows | written before row tagging existed; neither tag layout, returned verbatim | `decode_payload_row`'s fallback arm (`_ => Some(row.to_vec())`) |
| key shapes in `payloads` | content-addressed `fz://blob/<64-lowercase-hex>` (`try_put_content_ref`) vs named keys (`"read"`, `"search"`, `"ls_manifest"`, `read/ref`, view aliases `view_*/ref` / `r*/bytes`) and execution-scoped `fz://seq/<kind>/<n>` transients (LRU-pruned, never durable) | `try_put_content_ref`, `expand_current_store`, `recovery_key_priority`, `prune_transient_payloads` |
| ref-index shards | `~/.fszero/ref-index/<xx>.ndjson` lines `{ref_id, store_path, ts}` — cross-store recovery POINTERS, not a payload store | `ref_index_shard_path`, `append_ref_index`, `expand_from_ref_index` |
| canonical CAS | `<store root>/blobs/sha256/<xx>/<hash>`, raw bytes only | `src/core/cas.rs` |

Emitted ref shapes on the wire: `fz://blob/<hash>` (durable, ZeroRef v1),
`fz://seq/<kind>/<id>` (execution-scoped, expansion returns a corrective
`seq_ref_scoped` error), engine-owned `fz://codemode/…` / `fz://file/…`,
and bare named keys (legacy compatibility path in `expand_with_tiers`).

Migration scope: only full-hash `fz://blob/<64-lowercase-hex>` payload rows
carry content-addressed objects; everything else is skipped (counted
`skipped_nonblob`). Ref-index shards are not migrated — they hold absolute
store paths, and the blobs they point at live in some store's `payloads`
table (each store migrates its own rows).

## Migration into the canonical CAS (fszero-c6q.3)

API: `RecoveryStore::migrate_blobs_to_cas() -> Result<MigrationReport, String>`
(`src/core/recovery.rs`), reachable as `FSZeroSession::migrate_blobs_to_cas`
and on the CLI as `fszero migrate-cas [--root PATH]`. Contract tests:
`tests/cas_migration.rs`.

- **Enable**: create the `blobs/` directory under the effective store root
  (the explicit opt-in above), re-open the session so `CasStore::detect`
  attaches, then run `fszero migrate-cas`. Running without an attached CAS
  is a typed `cas_unattached` error — the migration never creates the
  opt-in for you. Note the store-root resolution rule
  (`zerostack_store.rs`): migration operates on the store the session
  opens at the EFFECTIVE store root. A pre-unified `.fszero/store.sqlite3`
  layout has no CAS attach point; to migrate one, first move the db (and
  its `store.sqlite3.pack[.gN]` / `.ast` sidecars) to
  `<store root>/fszero/store.sqlite3` — an explicit user action, like the
  opt-in itself.
- **What it does**: iterates `payloads` rows whose key is
  `fz://blob/<64-lowercase-hex>`, reads each through the existing
  digest-verifying path (`get_payload` decode — inline, packed, and legacy
  untagged rows — then the `verified_blob` whole-object digest check) and
  publishes verified bytes via `CasStore::put_prehashed`. A versioned
  manifest is written under store key `cas/migration` (JSON: `version`,
  `objects` keyed by FULL content hash with `{source: inline|packed, size,
  status: migrated|already|corrupt|missing}`, `counts`). Corrupt legacy
  bytes are recorded (`corrupt` + `note_integrity`), NEVER published and
  NEVER deleted; unreadable torn-pack rows are recorded `missing`.
- **Invariant (interruption safety)**: migration never deletes or rewrites
  any legacy row or pack byte — it only ADDS objects to the CAS and
  replaces its own `cas/migration` manifest. Killing it at any point leaves
  both tiers consistent; a re-run is a verified no-op for already-published
  objects (`already`) and resumes the rest. Repeated runs are idempotent.
- **Verify**: re-run `fszero migrate-cas` — expect `migrated=0` and
  `corrupt=0 missing=0`; damage details (never silently absorbed) are in
  `integrity_report()` and the manifest. Reads need no verification step of
  their own: dual-read precedence already serves CAS → legacy → ref-index
  with corruption terminal (see "Tier precedence and fallback" above).
- **Rollback / downgrade**: nothing to roll back — the legacy store is
  untouched and remains fully readable. Downgrade = simply stop attaching
  the CAS (remove the `blobs/` opt-in); every blob still serves from the
  legacy tiers.
- **Cleanup is out of scope**: deleting migrated legacy rows/pack bytes is
  a separate explicit user action (a future bead), never part of migration.
- **Identity**: FSZero's strict ZeroRef v1 parser rejects prefix/short
  hashes (fszero-c6q.2), and migration never creates prefix aliases —
  manifest keys and CAS object names are full 64-lowercase-hex only.
- **Privacy**: report and CLI output are counts only — no blob contents, no
  private absolute paths.
