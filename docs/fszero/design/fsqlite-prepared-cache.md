# fsqlite prepared-statement cache (FSZero sampling)

Bead: `fszero-fsqlite-prepared-cache-metrics-fudk`

## What is measured

fsqlite-core keeps a process-global prepared-statement LRU and atomics:

- `ParserHotPathProfileSnapshot.prepared_cache_hits`
- `ParserHotPathProfileSnapshot.prepared_cache_misses`

via `fsqlite_core::connection::hot_path_profile_snapshot()`.

These are **not** the RecoveryStore payload-ref `cache_hits` / `cache_misses`
returned by `RecoveryStore::metric_snapshot` (content expand cache).

The `fsqlite` facade crate (0.1.19) does **not** re-export the hot-path snapshot
API. FSZero depends on `fsqlite-core = "0.1.19"` solely to surface those counters.

## Env gate

Set `FSZERO_FSQLITE_PREPARED_CACHE_PROFILE=1` (also `true`/`yes`/`on`) to enable
fsqlite hot-path profiling. When enabled:

- `RecoveryStore::prepared_cache_metric_snapshot()`
- `prepared_cache_metrics()` / `prepared_cache_metrics_json()`
- `session.root_report()["fsqlite_prepared_cache"]`

report live hit/miss totals. Counters are process-global and accumulate across
stores/sessions until process exit or `reset_hot_path_profile()`.

## RecoveryStore SQL inventory (const vs format!)

### Stable `const` SQL (good prepared-cache reuse)

| Area | Examples |
|------|----------|
| `recovery/mod.rs` | `SQL_SELECT_PAYLOAD_KEYS`, `SQL_SELECT_PAYLOAD_KV`, `SQL_SELECT_PAYLOAD_EXISTS`, `SQL_INSERT_PAYLOAD_KV`, and the bulk of payload/LRU DDL |
| `mutation_log.rs` | `SQL_NEXT_MUTATION_SEQ`, `SQL_INSERT_MUTATION`, `SQL_MUTATIONS_AFTER` |
| `worlds.rs` | `SQL_SELECT_ACTIVE_WORLDS`, `SQL_SELECT_WORLD_EDITS`, `SQL_INSERT_WORLD_*`, `SQL_UPDATE_WORLD_STATE` |
| `chunk_index.rs` | `SQL_SELECT_CHUNK`, `SQL_INSERT_CHUNK`, `SQL_DELETE_FILE_CHUNKS`, `SQL_INSERT_FILE_CHUNK`, `SQL_SELECT_FILE_CHUNKS` |
| `edit_intent.rs` | mostly literal `INSERT`/`DELETE`/`UPDATE` strings |

### `format!`-built SQL (prepared-cache poison risk)

| Site | Pattern | Risk |
|------|---------|------|
| `mutation_log.rs:63-65` | `format!("SELECT {MUTATION_ROW_COLS} FROM mutation_log WHERE …")` | Three string variants; column list is a fixed const fragment, so only a small finite set of strings -- LRU still reusable if the full string is identical across calls |
| `durable_integrity.rs` salvage | `format!("SELECT COUNT(*) FROM {quoted}")`, `format!("SELECT {column_list} FROM {quoted} NOT INDEXED")` | Identifier-quoted table/column names make **per-table** distinct SQL -- expected miss growth on multi-table salvage; not on the hot payload path |

## How to sample after a bulk workload

```bash
export FSZERO_FSQLITE_PREPARED_CACHE_PROFILE=1
# in-process: RecoveryStore put/expand loops, then prepared_cache_metrics()
# or: inspect session.root_report()["fsqlite_prepared_cache"]
```

Unit evidence: `prepared_cache_metrics_accumulate_on_repeated_payload_ops` in
`src/core/recovery/mod.rs` (RCH: `cargo test -p fs-zero prepared_cache_metrics_accumulate`).

## Dedup notes

- Not payload `metric_snapshot` cache hits
- Not upstream fsqlite savepoint/CellSlotCache issues
- Contrasts with `ast_store.rs` rusqlite `prepare_cached` (explicit) vs recovery
  fsqlite path (engine-side LRU, now measurable)
