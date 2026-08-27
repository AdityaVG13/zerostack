# Decision: graphzero-4u80 — bigram search default-on bakeoff

**Date:** 2026-07-16  
**Bead:** `graphzero-4u80`  
**Prior:** `graphzero-vurp` / [003-search-bigram-spike-vurp.md](003-search-bigram-spike-vurp.md)  
**Verdict:** **REJECT default-on (BLOCKED — no search-scale gold corpora)**

## Intent

Re-bench `GRAPHZERO_SEARCH_BIGRAM` on committed gold corpora + head-to-head bakeoff success-rate before flipping the flag default-on. Sister of FSZero `fszero-kbo`.

## What was checked

| Artifact | Role | Status for search bakeoff |
|----------|------|---------------------------|
| `benchmarks/gold/corpora/` | Committed "gold" | **Edge-accuracy excerpts only** (~2.4 KB total; 4 fixtures). Not search-scale. |
| `benchmarks/gold/edges.jsonl` | Edge precision gold | Call/import/implements rows — no rare/common/absent search query set. |
| `benchmarks/head_to_head_bakeoff/` | Competitor bakeoff scaffold | Index/blast/correctness axes — no search-latency success-rate gate for bigram. |
| `benchmarks/search_bigram_spike_result.json` | vurp synthetic spike | ~8.5k synthetic symbols; **not** committed gold. Must not be re-published as gold evidence. |
| `GRAPHZERO_SEARCH_BIGRAM` default | Product flag | Remains **off** (opt-in `=1`). |

Corpus byte inventory (2026-07-16):

```
318  foreign-rust-thiserror/display_excerpt.rs
594  foreign-ts-zod/api_excerpt.ts
503  graphzero/extract_engine_excerpt.rs
973  graphzero/traits_excerpt.rs
----
2388 total
```

## Gates (unchanged from vurp; cannot re-measure on gold)

| Gate | Required | Gold re-bench | Pass? |
|------|----------|---------------|-------|
| Exact hit eligibility | preserved | not run (no corpus) | — |
| Bakeoff success rate | no regress | no search axis on bakeoff | — |
| p95 search latency | ≥30% improve | **blocked** | — |
| Memory | ≤1.25× | **blocked** | — |
| Cold/warm index | ≤10% regress | **blocked** | — |

Prior synthetic reject (vurp, do not treat as gold): p95 **+28.0%** (<30%), mem **~2.24×** (>1.25×). Follow-ups `graphzero-mba0` (densify) and `graphzero-lrin` (publish-time index) remain open.

## Decision

**Do not enable by default.** Do **not** invent gold numbers from the synthetic spike or from edge-excerpt fixtures.

Ship remains: spike behind `GRAPHZERO_SEARCH_BIGRAM=1`, default off.

## Child work

**`graphzero-aluu` shipped** search-scale gold + harness — see
[005-search-bigram-gold-aluu.md](005-search-bigram-gold-aluu.md). Default-on
decision remains closed/REJECT until gold bakeoff gates + densify/publish-time
follow-ups clear.

## How to try (unchanged)

```bash
GRAPHZERO_SEARCH_BIGRAM=1 graphzero search needle
```
