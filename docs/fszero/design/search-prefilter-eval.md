# Search prefilter evaluation (fszero-9yq / 9ot / up8 / kbo)

**fszero-kbo decision: `ACCEPT`** — default → `bigram_memmem` (escape: `FSZERO_SEARCH_PREFILTER=contains`).

## Production default

| Env | Behavior |
| --- | --- |
| unset / `bigram_memmem` | lazy incremental bigram filter + `memchr::memmem` |
| `FSZERO_SEARCH_PREFILTER=contains` | `direct_literal_scan` uses `str::contains` |

When bigram path is active:

- First query `ensure_files` fills missing bigram bitsets from disk (lazy).
- Incremental `ingest_file` upserts from bytes already loaded for extract.
- Watch/remove and index rebuild drop stale entries.
- **Never** bulk-rebuilds the whole corpus at `build_index` time (9yq REJECT).

## Acceptance gates (fszero-9ot / up8; still hold for kbo)

- rare/absent search p50 improve ≥ 25% (preserve 9yq warm gains)
- common p50 regress ≤ 10%
- cold ingest regress ≤ 20% (`from_bytes` during read+AST extract)
- memory ≤ 1.25× RSS after materializing the lazy bigram index
- watch upsert create/modify hit parity + delete absent; upsert p95 ≤ 1ms
- exact hit-set parity with baseline (enforced in spike + unit tests)
- no fuzzy ranking / mmap scope

## Measured outcome

See [`benchmarks/search-prefilter-spike.md`](../../benchmarks/search-prefilter-spike.md) (artifact tip at kbo run: `12dff67327cd37d9028479192956e20512187cd1`).

### Prior REJECT (fszero-9yq)

Bulk materialization failed amortization (`build_bigrams` ~4.16× `read_all`).
Incremental ingest accounting (9ot) cleared the cold gate.

### fszero-up8

Wired `scan_bigram_memmem` into `direct_literal_scan` behind `FSZERO_SEARCH_PREFILTER=bigram_memmem` (opt-in first land).

### fszero-kbo result

Re-bench on gold spike corpus cleared 9ot/up8 gates plus watch upsert. Production default flipped to `bigram_memmem`; `contains` remains an escape hatch.

## History

- `fszero-9yq`: REJECT bulk amortization
- `fszero-9ot`: ACCEPT incremental ingest
- `fszero-up8`: wire opt-in production path
- `fszero-kbo`: default-on bakeoff (this doc)
