# Index trust model: staleness detection and first-run-only cold semantics (fszero-krl)

How FSZero knows the index is trustworthy, and what happens when it is not.
Cold indexing is a first-run-only concept; everything after is delta work.

## The three staleness defenses

| defense | mechanism | cost |
| :-- | :-- | :-- |
| startup divergence scan | every `build_index` diffs the on-disk `(mtime, len)` signature of EVERY tracked file against the persisted manifest (`INDEX_MANIFEST_KEY`); only dirty/new/removed files are re-ingested -- a targeted delta scan, never a rebuild | one stat per file (541ms at 100k files) |
| per-op stat refresh | when no watcher is attached, ops that consult the index re-stat known files and re-ingest drift before answering (`refresh_stale_index_files`) | O(known files) stats per op |
| watch mode (`FSZERO_WATCH=1`) | FSEvents/inotify events applied incrementally at op boundaries; per-op stat scan short-circuits **only while** `watch_index_trusted()` (no backlog/overflow/truncated rescan). Untrusted → metadata refresh runs | p50 713us per save when trusted (release) |

## Save-to-queryable freshness (fszero-4y2)

Watch drains call `reindex_path` for each dirty file. Since fszero-4y2.1 the
persist path is an AST-diff span upsert: compare prior symbol/import rows to
the new extraction and DELETE/INSERT only changed/added/removed spans so
unchanged chunks stay put (call edges remain a file-scoped replace). No full
rebuild. The measured agent-visible path is:

`save -> WatchEvent::Path -> drain at op boundary -> search hit`

Evidence (injection harness, 200-file warm index, 50 unique markers;
debug build measured on this machine: p50 ≈ 8.6ms / p95 ≈ 11.7ms):

| metric | gate | test |
| :-- | :-- | :-- |
| save→queryable p50 | < 1s | `save_to_queryable_freshness_under_1s_p50` in `tests/watch_mode.rs` |
| save→queryable p95 | < 1s | same |
| per-save index apply p50 | < 1ms (release) | `per_save_index_cost_under_1ms_p50` (fszero-k14) |
| AST-diff single-symbol writes | << clear+rewrite | `upsert_spans_diff_rewrites_only_changed_rows` in `ast_store` |

True AST-diff span upsert shipped in fszero-4y2.1; fszero-4y2 shipped the
file-level watch path + freshness gate.

## Liveness and generation

- Watcher liveness: `FSZeroSession::watch_active()` plus
  `watch_stats()` counters (events_seen, files_updated, files_removed,
  rescans, drains, rescan_priority_drains, truncated_rescans), surfaced in
  `telemetry.extra.watch`.
- Reconcile FSM (`watch_reconcile_state` / `watch_index_trusted`):
  - **drain_backlog**: Path storm hit drain cap; root Rescan forced.
  - **overflow_pending**: Rescan/overflow not yet completed trusted.
  - **untrusted_removals**: truncated walk skipped removal detection.
  - Rescan events are drained with priority over Path FIFO (fszero-w2g.2).
- Generation counter: `ast_generation` (persisted in the manifest) tags
  every AST row; readers query rows for their generation only, so a torn
  or superseded generation can never satisfy a fresh query.

## If the watcher was down

Events missed while a watcher was dead are recovered by the SAME startup
divergence scan on the next session (or the per-op refresh in a live one):
a targeted delta of exactly the drifted files. Watcher overflow/errors in a
live session trigger a scoped subtree sig-diff rescan (`WatchEvent::Rescan`)
-- removal detection disables itself on truncated walks rather than
guessing.

## Never silent

- A full rebuild happens in exactly two cases, both loud or expected:
  first run (no manifest -- cold by definition) and manifest-without-rows
  (store wiped / persist torn mid-write): stderr says so and names the file
  count before rebuilding.
- A silently stale answer requires all three defenses to miss, i.e. a file
  changed without its (mtime, len) changing AND no watch event -- the same
  blind spot make/cargo accept; content hashing on every op is the
  deliberate non-goal.
