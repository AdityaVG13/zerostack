# Durability and loud-failure contract (fszero-uv0)

Torn or corrupted store state is REPORTED, never silently partial. This
page is the user-facing contract; the enforcement lives in
`src/core/recovery.rs` (fszero-ku8) and is proven byte-by-byte in
`tests/crash_injection.rs` (fszero-fmz; nightly full-offset sweep via
`cargo test --test crash_injection -- --ignored`).

## What is verified, when

| surface | integrity mechanism | on damage |
| :-- | :-- | :-- |
| `fz://blob/<sha256>` reads (any tier) | full digest verified BEFORE bytes are served -- local store, pack sidecar, and cross-store ref-index recovery | reported + treated as missing so an outer tier may supply a good copy; never served |
| pack sidecar (`store.sqlite3.pack`) | committed locator with unreadable bytes = torn tail detection | `torn_pack` report; everything before the tear stays readable |
| ref-index shards (`*.ndjson`) | per-line JSON parse; a torn tail invalidates only the damaged lines | `ref_index_damaged` report with count; valid prefix keeps serving |
| main store (`store.sqlite3`, fsqlite WAL) | sqlite transactional atomicity; open failure degrades the session | `durable_degraded: true` + stderr notice; ops keep working in-memory, journaling disabled |
| AST sidecar (`store.sqlite3.ast`), `.asgrep/index.db` | real-sqlite txn integrity; both are REBUILDABLE caches | damage costs one cold rebuild, never data |
| index manifest | parse failure or manifest-without-rows | one full cold rebuild (documented recovery, not an error) |

## Error taxonomy (expand paths)

`RecoveryStore::expand_with_tiers` returns typed errors, in order of
diagnosis:

- `seq_ref_scoped: <ref>` -- execution-scoped ref, never durable; corrective
  guidance included.
- `ref_unrecoverable: <ref> (corrupt_payload: ... sha256 mismatch ...)` --
  the bytes exist but fail their digest; they were NOT served.
- `ref_not_found: <ref> (tiers tried: ...)` -- clean miss, tiers named.

## The report channel

`RecoveryStore::integrity_report() -> (violations_seen, last_detail)` --
monotonic counter plus the most recent damage detail
(`corrupt_payload` | `torn_pack` | `ref_index_damaged`), never silently
cleared. Ops that hit damage still return their normal `<op>:0 (...)`
acks (visible `X0`); the report is the forensic detail.

## Recovery guidance

| symptom | action |
| :-- | :-- |
| `ref_unrecoverable` / `corrupt_payload` | the local copy is damaged; if a teammate store holds the blob the ref-index tier recovers it automatically. Otherwise the content is gone -- restore the file from the tree/git and re-run the op. |
| `torn_pack` | a crash tore the pack tail; unreferenced garbage is harmless, referenced-but-torn payloads are reported per-read. Rebuild derived state with a cold index; journaled history before the tear is intact. |
| `ref_index_damaged` | shard compaction (automatic on next append past 1MB) rewrites the shard atomically and drops the damaged lines. |
| `durable_degraded: true` | the store file failed to open (torn db). Move it aside; the next run recreates it. fs.history/undo for the damaged period is lost and SAYS so -- ops never pretend to journal. |

Non-goals: the store is a derived cache plus a mutation journal -- it never
holds the only copy of repo content (pre/post blobs are content-addressed
copies; the tree itself is the source of truth).

## Absolute-durable barrier class


FSZero markets **absolute-durable** recovery storage for refs, memory, and
worlds. That claim is bound to a measured barrier class, not marketing prose.

## Barrier class

| surface | mechanism | notes |
| --- | --- | --- |
| Main store (`store.sqlite3`) | `PRAGMA journal_mode=WAL` + `PRAGMA synchronous=FULL` | Every commit fsyncs; survives process kill and power loss assuming the disk honors `fsync` |
| Pack sidecar (`store.sqlite3.pack`) | `path::full_sync_file` after append (`sync_all` + macOS `F_FULLFSYNC`), **before** the SQLite locator row commits | Data + metadata; same barrier class as workspace `atomic_write`; `sync_data` alone is **not** this class |
| CAS (`blobs/sha256/…`) | `path::full_sync_file` on temp object before rename-publish; unix shard-dir fsync after rename | Content-addressed objects; fail-loud on barrier I/O |
| Fail-closed open | `try_with_repo_store`; in-memory only with `FSZERO_ALLOW_EPHEMERAL=1` | Servers never silently degrade to ephemeral |

Ordering invariant: pack bytes hit stable storage, then the locator becomes
visible in SQLite. Crash between append and sync may orphan a pack tail
(harmless). Crash after ack cannot leave a committed locator at short bytes.

### Execution-scoped rows are outside this class

`fz://seq/...` payloads (the CodeMode response bundle, and anything else put
under a transient key) are **never** written to the pack, at any size. They are
stored inline and committed in the same transaction as their key.

This is not a weakening: the class was never claimed for them. `expand_with_tiers`
refuses every `://seq/` ref before it reaches a tier -- "Execution-scoped refs are
never durable" -- and a transient put does not survive a reopen at any size. They
are LRU-pruned scratch read back through the in-session stash.

The pack costs one `full_sync_file` barrier per write (`sync_all` + macOS
`F_FULLFSYNC`), ahead of the SQLite commit barrier that follows it. On macOS
both are `F_FULLFSYNC` (~4ms each, measured), so routing
this key class through the pack spent a guaranteed disk barrier per CodeMode plan
buying ordering for data that has no durability guarantee to order (fszero-5u7).
Covered by `transient_put_stays_inline_off_the_pack` and, for the converse,
`non_transient_put_of_same_size_still_packs`.



### Measuring CAS put/get barriers

Set `FSZERO_CAS_PHASES=1` to emit one JSON line on stderr per successful CAS
`put`/`get` with `cas_phases_us` (put: `write`, `full_sync`, `rename`,
`dir_sync`; get: `read`, `verify`), plus `bytes` and `total_us`. Default off
(fszero-1q7n).

### Measuring durable packed-put barriers

Set `FSZERO_DURABLE_PUT_PHASES=1` to emit one JSON line on stderr per
successful packed/immediate put commit (and pending flush) with
`durable_put_phases_us.pack_sync_us`, `commit_us`, `pack_dirty`, and `bytes`.
Default off. Attributes the pack `full_sync_file` barrier vs SQLite FULL
COMMIT (fszero-atup).

### Measuring workspace atomic_write barriers

Set `FSZERO_ATOMIC_WRITE_PHASES=1` to emit one JSON line on stderr per
successful `atomic_write_with_outcome` with `atomic_write_phases_us` for
`prepare_dir_sync`, `temp_write`, `full_sync`, `rename`, and `dir_sync`
(microseconds). Default off (zero cost when unset). Used to attribute APFS
`F_FULLFSYNC` cost vs write/rename on edit/world paths (fszero-1unf).

## What tests prove

- `durable_store_sets_synchronous_full`: pragma is FULL (2), not NORMAL (1)
- `durable_store_pragma_synchronous_is_full`: same check via reopen
- `acked_packed_put_survives_reopen`: after put returns, drop+reopen expands byte-exact
- `mid_pack_orphan_tail_without_locator_is_harmless`: raw pack append without a locator does not invent payloads
- `pack_truncation_reported_earlier_payloads_intact`: committed locator past EOF misses; prior payloads intact
- `torn_pack_surfaces_pack_torn_on_expand`: typed `pack_torn:` on expand_with_tiers

See `tests/crash_injection.rs` and `src/core/recovery.rs` unit tests.

## Non-goals

- Disks that lie about `fsync` (some cloud volumes) are outside this class.
- Derived indexes (AST sidecar) remain rebuildable caches, not the durability root.
- **Multi-file workspace publish** (`commit_world` across N paths) is **not** in
  this absolute-durable barrier class. Legal set L and the store-vs-workspace
  scoring table live in `docs/design/world-process-model.md` (fszero-k4ur.1 /
  jqf.5 workspace column).
