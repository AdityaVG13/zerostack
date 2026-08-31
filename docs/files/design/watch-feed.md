# Watch change feed

FSZero publishes a bounded, replayable change feed after a watch drain applies at least one real index change. ZeroStack may use the feed to repair derived structure and caches without making one engine import another.

## Contract

The feed is stored under `watch/feed`:

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

- `seq` is strictly monotonic, persisted, and resumes from `last_seq` after restart.
- `changed` covers creation and modification. Consumers re-read the file.
- `removed` also covers a path moved away. A rename appears as one removal and one change.
- `generation` identifies the file-index generation that accepted the event.
- The feed retains the newest 1,024 events.
- A consumer replays events after its stored sequence. If its sequence predates the retained ring, it performs a full rescan.
- Within one drain, changed paths precede removed paths and each group is sorted. Drains retain arrival order.
- Coalesced no-op events do not appear.

The feed is domain state, not a model-facing API. ZeroKernel exposes fresh structural results through `z.find` after ZeroStack settles the required repair.
