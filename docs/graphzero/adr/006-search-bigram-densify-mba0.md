# Decision: graphzero-mba0 — densify NameBigramIndex postings

**Date:** 2026-07-16  
**Bead:** `graphzero-mba0`  
**Prior:** `graphzero-f3jq` / [005-search-bigram-gold-aluu.md](005-search-bigram-gold-aluu.md)  
**Verdict:** **MEM GATE CLEARED / DEFAULT-ON STILL OFF**

## Intent

Gold bakeoff (`graphzero-f3jq`, `86ffbd9`) measured mem **2.61×** against the
`32 * symbol_count` heuristic (`memory_le_1_25x` red). Densify in-memory
`NameBigramIndex` postings toward ≤1.25× without flipping
`GRAPHZERO_SEARCH_BIGRAM` default-on.

## What shipped

`crates/graphzero-store/src/store/query/name_bigram.rs`:

- Packed **u16 byte-bigrams** (UTF-8 windows; still exact-safe for `contains`)
- **Singleton** `(key,id)` pairs + **CSR** for lists with len ≥ 2
- **Delta-varint** encoding of multi-list doc ids (`u16` first + uleb128 gaps)
- Path side table as raw **`[u8; 32]`** hashes (hex only on candidate return)

## Measurement (gold bakeoff, 3 rounds, release)

| Axis | f3jq (before) | mba0 (after) |
|------|---------------|--------------|
| mem ratio | **2.61×** | **0.67×** |
| `memory_le_1_25x` | false | **true** |
| approx_bytes | 1,885,876 | 484,517 |
| symbols | 22,544 | 22,544 |
| default-on | off | **off** |

p95 improve remains a separate gate (further latency work after publish-time
load via `graphzero-lrin`). Harness still records gates only; flag stays
opt-in `=1`.

## How to re-measure

```bash
flock /tmp/zerostack-swarm-locks/graphzero.lock \
  env CARGO_BUILD_JOBS=2 GRAPHZERO_SEARCH_GOLD_ROUNDS=3 \
  cargo test -p graphzero-query --test search_bigram_gold_bakeoff --release \
  -- --nocapture --test-threads=1
```
