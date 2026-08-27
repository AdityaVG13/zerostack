# Decision: graphzero-aluu — search-scale gold corpora + bakeoff harness

**Date:** 2026-07-16  
**Bead:** `graphzero-aluu`  
**Prior:** `graphzero-4u80` / [004-search-bigram-default-on-4u80.md](004-search-bigram-default-on-4u80.md)  
**Verdict:** **GOLD + HARNESS SHIPPED / DEFAULT-ON STILL OFF**

## Intent

Parent `graphzero-4u80` REJECTED default-on because committed gold was edge-accuracy
excerpts (~2.4 KB), not search-scale, and `head_to_head_bakeoff` had no search
latency axis. This bead lands the missing measuring stick.

## What shipped

| Artifact | Role |
|----------|------|
| `benchmarks/gold/search/` | Pinned search-scale gold (generator + corpus + SHA + queries) |
| `benchmarks/search_bigram_bakeoff/` | Driver + methodology + published `report.json` |
| `crates/graphzero-query/tests/search_bigram_gold_bakeoff.rs` | Focused release bakeoff (rare/common/absent, cold+warm, p95/mem) |

Scale floors (enforced): ≥200 files, ≥512 KiB, ≥10k indexed symbols.

Explicit non-gold: `benchmarks/search_bigram_spike_result.json` and edge excerpts
under `benchmarks/gold/corpora/`.

## What did **not** ship

- `GRAPHZERO_SEARCH_BIGRAM` default remains **off** (opt-in `=1` only).
- No ACCEPT of default-on; densify (`graphzero-mba0`) / publish-time index
  (`graphzero-lrin`) remain prerequisites if gold gates stay red.

## How to run

```bash
mkdir -p /tmp/zerostack-swarm-locks
flock /tmp/zerostack-swarm-locks/graphzero.lock \
  env CARGO_BUILD_JOBS=2 \
  python3 benchmarks/search_bigram_bakeoff/run.py
```

Focused test only — never a full `cargo test --workspace`.

## How to try the flag (unchanged)

```bash
GRAPHZERO_SEARCH_BIGRAM=1 graphzero search needle
```

## Measurement fill (`graphzero-f3jq`, 2026-07-16)

Ran focused `search_bigram_gold_bakeoff` (30 rounds, release). **Default-on still OFF.**

| Axis | Result |
|------|--------|
| success_rate | **1.0** (7/7 label matches) |
| warm batch p95 | bigram **16.35 ms** vs scan **14.93 ms** → **−9.56%** (gate ≥30% fail) |
| warm by class p95 (bigram / scan) | rare 0.89 / 0.86 ms; common 14.46 / 14.55 ms; absent 0.054 / 0.596 ms |
| cold first batch | **32.37 ms** (~126% over warm p50) |
| mem ratio | **2.61×** heuristic (~1.89 MB index / 22.5k symbols) — gate ≤1.25× fail |
| symbols / files / bytes | 22544 / 320 / 3169793 |

Gates `p95_improve_ge_30` and `memory_le_1_25x` were **red** at publish time.
`graphzero-mba0` later cleared `memory_le_1_25x` (see
[006-search-bigram-densify-mba0.md](006-search-bigram-densify-mba0.md)); p95 /
publish-time (`graphzero-lrin`) remain. No flip of `GRAPHZERO_SEARCH_BIGRAM`.
