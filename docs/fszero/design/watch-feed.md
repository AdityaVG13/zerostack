# Watch change feed v1 (fszero-lau)

The subscribable, replayable change feed sibling engines key off (graphzero
incremental graph updates, cachezero invalidation). Producer:
`publish_watch_feed` in `src/core/watch.rs`, emitted at the end of every
watch drain that applied at least one change.

## Contract (v1)

Store key `watch/feed` (expand it like any recovery key; in a unified
`.zerostack` store layout, sibling engines read the same store):

```json
{
  "version": 1,
  "last_seq": 4213,
  "events": [
    {"seq": 4212, "kind": "changed", "file": "src/a.rs", "generation": 7},
    {"seq": 4213, "kind": "removed", "file": "src/b.rs", "generation": 7}
  ]
}
```

- `seq` is strictly monotonic, persisted, and survives session restarts
  (resumed from the stored `last_seq`).
- `kind`: `changed` (created or modified -- consumers re-read the file) |
  `removed` (also covers moved-away; a rename is removed+changed pairs).
- `generation`: the index generation the event was applied under.
- Ring of the newest 1024 events. Snapshot+catchup semantics: a consumer
  stores its cursor (last consumed seq); if cursor >= events[0].seq it
  replays the tail; if its cursor is OLDER than the ring head it must full
  resync (walk the tree / re-derive) -- the feed says so via the gap.
- Ordering within one drain: all `changed` before all `removed`, each
  alphabetical; cross-drain ordering is drain order.
- Only real index changes are published: coalesced no-op events (same
  (mtime,len) sig) never appear.

Verified by `watch_feed_is_ordered_replayable_and_persistent` in
tests/watch_mode.rs.
