# Decision: graphzero-vurp — fff-style bigram candidate search

**Date:** 2026-07-16  
**Bead:** `graphzero-vurp`  
**Verdict:** **SPIKE SHIPPED / DEFAULT-OFF REJECT**

## What shipped

- `NameBigramIndex` in `graphzero-store` (char bigram postings over symbol names + paths).
- Cached on `Snapshot` via `OnceLock` (`name_bigram_index()`); daemon reopen replaces the Snapshot, so watch refresh stays coherent.
- `query_surface` `search` / `word` candidate filter behind **`GRAPHZERO_SEARCH_BIGRAM=1`** (default off).
- Exact `contains` eligibility preserved; iteration order unchanged; no fuzzy rerank (API does not expose fuzzy ranking today).
- Equivalence + head-to-head harness: `crates/graphzero-query/tests/search_bigram_spike.rs`
- Artifact: `benchmarks/search_bigram_spike_result.json`
- Criterion A/B hooks in `crates/graphzero-query/benches/query_surface.rs`

## Measurement (release, ~8.5k symbols, 8-query batch × 40 rounds)

| Metric | Scan (flag off) | Bigram (flag on, warm) |
|--------|-----------------|-------------------------|
| batch p50 | 9.30 ms | 7.22 ms |
| batch p95 | 11.57 ms | 8.33 ms |
| p95 improve | — | **+28.0%** |
| index build | — | 6.09 ms |
| approx index bytes | — | 573 KB (~2.24× naive 32B/symbol) |

Commands:

```bash
CARGO_BUILD_JOBS=2 cargo test -p graphzero-query --test search_bigram_spike --release -- --nocapture
```

## Acceptance vs bead gates

| Gate | Required | Observed | Pass? |
|------|----------|----------|-------|
| Exact hit eligibility | preserved | equivalence test green | yes |
| Deterministic labels | match flag off/on | `search_surface_labels_match_*` | yes |
| p95 search latency | ≥30% improve | **28.0%** | **no** |
| Memory | ≤1.25× | ~2.24× heuristic | **no** |
| Cold/warm index | ≤10% regress | build ~6 ms; warm OnceLock | n/a (new path) |
| Watch refresh | invalidate correctly | Snapshot replace clears OnceLock | yes (spike-level) |

## Decision

**Do not enable by default.** The spike is useful and correct but misses the p95 and memory gates. Keep `GRAPHZERO_SEARCH_BIGRAM` opt-in for further tuning.

## Follow-ups (filed)

1. Densify postings / packed u16 ASCII bigrams to cut memory toward ≤1.25× (`graphzero-mba0`).
2. Publish-time (or indexer) name-bigram section so cold queries avoid 6 ms build (`graphzero-lrin`).
3. Re-bench on committed gold corpora + head_to_head_bakeoff success-rate gate before flip — **`graphzero-4u80` REJECT / BLOCKED** (no search-scale gold; see [004-search-bigram-default-on-4u80.md](004-search-bigram-default-on-4u80.md)).
4. Optionally wire the same candidate filter into snap `first_symbol_name_containing` O(N) fallback.

## How to try

```bash
GRAPHZERO_SEARCH_BIGRAM=1 graphzero search needle
```
