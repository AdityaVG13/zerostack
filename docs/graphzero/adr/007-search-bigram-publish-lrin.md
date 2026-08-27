# Decision: graphzero-lrin — publish-time name-bigram sidecar

**Date:** 2026-07-16  
**Bead:** `graphzero-lrin`  
**Prior:** `graphzero-mba0` / [006-search-bigram-densify-mba0.md](006-search-bigram-densify-mba0.md)  
**Verdict:** **SIDECAR SHIPPED / DEFAULT-ON STILL OFF**

## Intent

Cold `OnceLock` build of `NameBigramIndex` inflated first-batch latency (gold cold
~32 ms vs warm p50). Persist densified postings at index publish so
`Snapshot::name_bigram_index()` loads bytes instead of rebuilding.

## What shipped

- Sidecar `shards/name_bigram_{snapshot_id:08}.bin` with magic **GZNB**, version **1**
- Written always from `write_snapshot` (same symbol table + paths as the global)
- Load-first in `name_bigram_index()`; missing file falls back to in-process build
  (legacy snapshots)
- Cleanup recognizes `name_bigram_` prefix
- `GRAPHZERO_SEARCH_BIGRAM` default remains **off** (opt-in `=1` only)

## Rollback

Older binaries ignore the sidecar file. Newer readers deny corrupt GZNB with a
loud error; missing sidecar silently falls back to build.

## How to verify (targeted only)

```bash
flock /tmp/zerostack-swarm-locks/graphzero.lock \
  env CARGO_BUILD_JOBS=2 \
  cargo test -p graphzero-store name_bigram --lib -- --nocapture

flock /tmp/zerostack-swarm-locks/graphzero.lock \
  env CARGO_BUILD_JOBS=2 GRAPHZERO_SEARCH_GOLD_ROUNDS=3 \
  cargo test -p graphzero-query --test search_bigram_gold_bakeoff --release \
  -- --nocapture --test-threads=1
```

## Spot check (2026-07-16, 3 rounds release)

After sidecar: cold first batch **~15.3 ms** (previously ~32 ms with OnceLock
build). Cold ≈ warm p50. Mem gate still green (~0.67×). Warm p95 improve still
red (separate from cold-path); default-on stays off.

Follow-up `graphzero-md9k` re-ran gold bakeoff and **REJECT**ed default-on — see
[008-search-bigram-default-on-md9k.md](008-search-bigram-default-on-md9k.md).