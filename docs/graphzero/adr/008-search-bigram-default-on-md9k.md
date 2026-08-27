# Decision: graphzero-md9k — gold bigram bakeoff after mba0 + lrin

**Date:** 2026-07-16  
**Bead:** `graphzero-md9k`  
**Prior:** `graphzero-mba0` (ca19cb0) / `graphzero-lrin` (24f042c)  
**ADRs:** [006](006-search-bigram-densify-mba0.md), [007](007-search-bigram-publish-lrin.md)  
**Verdict:** **REJECT default-on**

## Intent

Re-measure gold search bigram bakeoff after densify + publish-time GZNB sidecar.
Accept `GRAPHZERO_SEARCH_BIGRAM` default-on only if all gates clear; otherwise leave
opt-in.

## Gates (required)

| Gate | Required | Result | Pass? |
|------|----------|--------|-------|
| `success_rate` | 1.0 | **1.0** (7/7) | yes |
| `memory_le_1_25x` | ≤1.25× | **0.67×** | yes |
| `p95_improve_ge_30` | ≥30% vs scan | **−7.10%** | **no** |

## Numbers (post-mba0 + lrin; `report.json`)

| Axis | Value |
|------|-------|
| warm batch p95 bigram / scan | 15.36 ms / 14.35 ms |
| p95 improve | **−7.10%** (bigram slower than scan) |
| mem ratio | **0.67×** (`memory_le_1_25x` green) |
| cold first batch | **15.29 ms** (≈ warm p50; sidecar effect) |
| success_rate | **1.0** |
| symbols / files / bytes | 22544 / 320 / 3169793 |

## Decision

**Do not enable by default.** `GRAPHZERO_SEARCH_BIGRAM` stays opt-in (`=1` only).
Escape hatch / disable path unchanged (unset or non-truthy env). Remaining gap is
warm p95 latency vs linear scan on the gold common-query batch — densify and
sidecar cleared mem + cold, not the ≥30% improve gate.

## How to try (unchanged)

```bash
GRAPHZERO_SEARCH_BIGRAM=1 graphzero search needle
```
