# Search bigram gold bakeoff methodology (graphzero-aluu)

Honest head-to-head of scan (`GRAPHZERO_SEARCH_BIGRAM` unset) vs indexed
candidate filter (`GRAPHZERO_SEARCH_BIGRAM=1`) on the **search-scale gold**
corpus in `benchmarks/gold/search/`.

## Explicit non-inputs

| Artifact | Why excluded |
|----------|--------------|
| `benchmarks/search_bigram_spike_result.json` | Synthetic spike (~8.5k funcs); not gold |
| `benchmarks/gold/corpora/*_excerpt.*` | Edge-accuracy excerpts (~2.4 KB) |

## Axes recorded

1. **Success rate** — label-set equality scan vs bigram for every gold query.
2. **p50 / p95** — per class (rare / common / absent) and batch, cold + warm.
3. **Memory** — `NameBigramIndex::approx_bytes` vs `32 * symbol_count` heuristic
   (same ratio published in the vurp spike ADR; not RSS unless extended later).
4. **Cold vs warm** — cold includes first `name_bigram_index()` build after
   `Snapshot::open`; warm is post-`OnceLock` steady state.

## Gates (decision thresholds; harness records pass/fail, does not flip default)

| Gate | Required |
|------|----------|
| Exact hit eligibility / label match | 100% success rate |
| p95 search latency (warm batch) | ≥ 30% improve vs scan |
| Memory | ≤ 1.25× heuristic |
| Cold index build vs warm | document; ≤ 10% warm regress vs prior warm baseline when comparable |

## How to run

```bash
mkdir -p /tmp/zerostack-swarm-locks
flock /tmp/zerostack-swarm-locks/graphzero.lock \
  env CARGO_BUILD_JOBS=2 \
  python3 benchmarks/search_bigram_bakeoff/run.py
```

`run.py` invokes `cargo` directly (no inner flock). Hold the lock at the
outer wrapper only — nesting `flock` on the same path deadlocks.

Focused cargo only — never a full workspace suite from this driver.

## Outputs

- `benchmarks/search_bigram_bakeoff/report.json` — published gold bakeoff numbers
- Does **not** mutate product default for `GRAPHZERO_SEARCH_BIGRAM`
