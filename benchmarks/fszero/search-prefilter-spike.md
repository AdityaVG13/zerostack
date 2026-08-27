# Search prefilter spike (fszero-kbo default-on bakeoff)

- **Decision:** `ACCEPT`
- **Default-on:** `FLIP default to bigram_memmem`
- **Winner:** `bigram_memmem`
- **Bead:** `fszero-kbo`
- **Commit tip at run:** `12dff67327cd37d9028479192956e20512187cd1`
- **Dirty:** `True`
- **Hardware:** Apple M5 Max / 48 GB
- **Date:** 2026-07-16T20:42:02Z
- **Corpus:** 1201 files / 104857680 bytes

## Query p50 (ms)

| label | baseline | memmem | bigram warm | rare/absent gain mem / big |
| --- | ---: | ---: | ---: | --- |
| rare | 10.723 | 10.457 | 0.102 | +2.5% / +99.0% |
| common | 2.432 | 2.515 | 2.509 | -3.4% / -3.2% |
| absent | 10.778 | 10.573 | 0.057 | +1.9% / +99.5% |
| ascii | 10.470 | 10.747 | 0.102 | -2.6% / +99.0% |
| unicode | 10.717 | 11.042 | 0.091 | -3.0% / +99.1% |

## Cold ingest (lazy incremental — fszero-9ot gate)

Per-file `BigramBitset::from_bytes` during read+AST extract (mirrors `ingest_one_file`), not bulk rebuild-vs-read-all.

- baseline ingest (read+extract): **13176.031 ms**
- with bigram upsert: **13314.653 ms**
- from_bytes sum (instrumented): **53.511 ms**
- from_bytes p50 / p95: **43.8 / 48.5 µs**
- **cold_ingest_regress: 0.41%** (from_bytes_sum / baseline_ingest; gate ≤20%)

## Watch upsert (fszero-kbo)

- K=10 create / modify / delete on warm lazy index
- create upsert p50/p95: **53.2 / 176.4 µs**
- modify upsert p50/p95: **48.2 / 81.3 µs**
- delete remove p50/p95: **23.6 / 79.9 µs**
- parity_ok=True deleted_absent_ok=True (create_hits=10 modify_hits=10)

## Gates

```json
{
  "memmem": {
    "rare_absent_p50_improve_ge_25": false,
    "common_regress_le_10": true,
    "cold_ingest_regress_le_20": true,
    "memory_le_1_25x": true,
    "watch_upsert_parity": true,
    "watch_upsert_cost_p95_le_1ms": true,
    "rare_improve_pct": 2.4810411455495887,
    "absent_improve_pct": 1.9032118979761574,
    "common_improve_pct": -3.442235191167671
  },
  "bigram_memmem": {
    "rare_absent_p50_improve_ge_25": true,
    "common_regress_le_10": true,
    "cold_ingest_regress_le_20": true,
    "memory_le_1_25x": true,
    "watch_upsert_parity": true,
    "watch_upsert_cost_p95_le_1ms": true,
    "rare_improve_pct": 99.04566445191709,
    "absent_improve_pct": 99.4719068295083,
    "common_improve_pct": -3.193776441406183,
    "cold_ingest_regress_pct": 0.4061230264261178,
    "baseline_ingest_ms": 13176.030541999999,
    "with_bigram_ingest_ms": 13314.653416000001,
    "from_bytes_sum_ms": 53.510894,
    "from_bytes_p50_us": 43.833,
    "from_bytes_p95_us": 48.458999999999996,
    "rss_ratio": 1.0061728395061729,
    "bigram_index_approx_bytes": 9953888,
    "bulk_proxy_cold_regress_pct": 342.964473916146,
    "watch_upsert": {
      "k": 10,
      "create_upsert_p50_us": 53.208999999999996,
      "create_upsert_p95_us": 176.417,
      "modify_upsert_p50_us": 48.25,
      "modify_upsert_p95_us": 81.29199999999999,
      "delete_remove_p50_us": 23.583000000000002,
      "delete_remove_p95_us": 79.916,
      "parity_ok": true,
      "deleted_absent_ok": true,
      "create_hit_count": 10,
      "modify_hit_count": 10
    }
  }
}
```

## Bulk proxy (fszero-9yq REJECT reference)

```json
{
  "event": "amortization_bulk_proxy",
  "note": "fszero-9yq REJECT proxy; not the fszero-9ot gate",
  "read_all_ms": 16.020792,
  "build_bigrams_ms": 70.966417,
  "build_over_read_ratio": 4.42964473916146
}
```

## Verdict notes

ACCEPT default-on: cold ingest regress 0.41% ≤20%; rare/absent +99.0% / +99.5%; common -3.2%; watch upsert parity+cost hold.
