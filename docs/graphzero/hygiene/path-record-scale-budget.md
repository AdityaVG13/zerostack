# Path-record scale budget (design envelope; unmeasured)

Status: **advisory / unmeasured**. This document defines the measurement
envelope for in-memory path-record load and lookup. It does **not** claim
benchmark numbers.

## Types and call sites (actual code)

| Item | Location |
| --- | --- |
| `PathRecord` | `crates/graphzero-store/src/store/query/types.rs` -- `mtime_nanos`, `size`, `tier_bits`, `path: String` |
| Snapshot map | `crates/graphzero-store/src/store/query/snapshot.rs` -- `paths: HashMap<ContentHash, PathRecord>` |
| Load from sidecar | `crates/graphzero-store/src/store/query/legacy.rs` -- `load_path_records(shards_dir, snapshot_id)` reads `paths_*.txt` |
| Lookup by rel path | `crates/graphzero-store/src/store/query/freshness.rs` -- `path_record_for_rel` |
| Memory / drift | `crates/graphzero-store/src/store/memory.rs` -- freshness / path drift using `PathRecord` |
| Iterate | `Snapshot::path_records()` / `path_for_blob` in `snapshot.rs` |

Keys are `ContentHash` (32 bytes), not 64-hex `String`s (see comment in
`load_path_records` / graphzero-a4t6p). Values still own a `path: String`
per blob.

## Scale envelope (thresholds advisory / unmeasured)

Use these columns when a future Spark measurement lands. Until then, every
numeric cell is a **placeholder band**, not a gate.

| Dimension | Small | Medium | Large (stress) | Notes |
| --- | ---: | ---: | ---: | --- |
| Path count (`paths.len()`) | 1e3 | 1e5 | 1e6 | One record per content-addressed blob path line |
| Sidecar bytes on disk | ~100 KiB | ~10–50 MiB | ~100+ MiB | Depends on path string lengths |
| Resident map bytes (RSS delta) | **unmeasured** | **unmeasured** | **unmeasured** | HashMap + `PathRecord` + path strings |
| `path_for_blob` / hash lookup p50 | **unmeasured** | **unmeasured** | **unmeasured** | `HashMap` by `ContentHash` |
| `path_record_for_rel` p50 | **unmeasured** | **unmeasured** | **unmeasured** | May scan / secondary index -- measure separately |
| Cold `load_path_records` p50 | **unmeasured** | **unmeasured** | **unmeasured** | Parse `paths_*.txt` into the map |

**Do not** treat the Small/Medium/Large path counts as pass/fail CI gates
until a measured baseline exists with corpus commit + host id recorded.

## Proposed measurement method (future; not run in this bead)

1. Host: Spark via RCH; `CARGO_TARGET_DIR=/tmp/rch_target_graphzero`.
2. Corpus: fixed repo snapshot with known path-sidecar line count (record
   `wc -l` of `paths_XXXXXXXX.txt` and file size).
3. Ops: (a) `load_path_records` / snapshot open; (b) N random
   `path_for_blob` hits; (c) N `path_record_for_rel` hits on existing paths.
4. Report: path count, sidecar bytes, RSS delta (or allocator bytes), p50
   lookup ns/us -- all labeled **measured** with command lines.
5. Only then promote bands above into ratchet gates if desired.

## Non-claims

- No fabricated p50 or byte numbers.
- No change to `PathRecord` layout in this bead.
- Memory path in `memory.rs` is a consumer of the map, not a separate
  on-disk format.
