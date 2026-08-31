# Structure crash-boundary corpus

Each JSONL row names a crash point and the recovery contract that must hold after restart. The initial corpus covers indexing, shard writes, sidecar appends, and semantic reservations.

Executable tests use the `evidence.test` names as stable anchors.
