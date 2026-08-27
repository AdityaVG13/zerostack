# Decision: graphzero-2tee — warm p95 gap after selectivity fallback

**Date:** 2026-07-16  
**Bead:** `graphzero-2tee`  
**Prior:** `graphzero-md9k` REJECT (d42616d, p95 −7.10%)  
**Verdict:** **REJECT default-on** (p95 improve **+10.20%**, need ≥30%)

## Intent

Close the warm batch p95 gap for name-bigram vs scan on search-scale gold
(≥30% improve) without flipping `GRAPHZERO_SEARCH_BIGRAM` until green.

## Root cause (common class dominates)

Gold common needles (`parse`, `parse_alpha_`) hit ~35% of symbols
(`rarest_posting` ≈ 7638 / 22544). With `budget=80`, linear scan fills the
budget after ~230 `contains` checks. Materializing a multi-k candidate set
(HashSet intersect + path hex HashSet) is strictly slower than that short
scan — this is why md9k measured **−7.1%**.

Absent/rare already win with bigram; batch p95 is dominated by common.

## What shipped (still opt-in)

1. **Selectivity early-out** (`name_bigram.rs`): if rarest posting >25% of
   universe or `> budget * 64`, return `None` → search falls back to scan.
2. **Iterate sorted candidate `Vec<u32>`** instead of `0..N` + HashSet probe.
3. **Lazy path candidates**: build path hex set only when symbol search did
   not fill the budget.
4. **Bakeoff harness**: remove nested `flock` inside `run.py` (outer flock
   only; nested same-path flock deadlocks).

`GRAPHZERO_SEARCH_BIGRAM` remains **opt-in** (`=1` only). Not flipped.

## Gates (gold bakeoff, 30 rounds, release)

| Gate | Required | Result | Pass? |
|------|----------|--------|-------|
| `success_rate` | 1.0 | **1.0** | yes |
| `memory_le_1_25x` | ≤1.25× | **0.67×** | yes |
| `p95_improve_ge_30` | ≥30% vs scan | **+10.20%** | **no** |

## Numbers vs md9k

| Axis | md9k (d42616d) | 2tee (this) |
|------|----------------|-------------|
| warm batch p95 bigram / scan | 15.36 / 14.35 ms | **12.28 / 13.67 ms** |
| p95 improve | **−7.10%** | **+10.20%** |
| common bigram / scan p95 | 14.38 / 12.81 | **12.32 / 12.50** (≈scan; fallback) |
| rare bigram / scan p95 | 0.85 / 0.76 | **0.21 / 0.80** |
| absent bigram / scan p95 | 0.05 / 0.56 | **0.04 / 0.56** |
| mem ratio | 0.67× | 0.67× |
| `parse*` scan_fallback | n/a | **true** |

## Decision

**Do not enable by default.** Selectivity closed the common-class regress and
yields ~+10% batch p95, but gold’s dense `parse*` + low budget structurally
cap headroom well below +30% unless the gold query mix or budget model
changes. Flag stays opt-in.

## How to re-measure

```bash
flock /tmp/zerostack-swarm-locks/graphzero.lock \
  env CARGO_BUILD_JOBS=2 \
  python3 benchmarks/search_bigram_bakeoff/run.py
```
