# Objective hygiene (resolve ranking + telemetry KPIs)

Normative pins for optimization objectives. Prevents Goodharting proxy scores
into product "proof." Math-review run:
`.math-review/runs/20260717T052725Z-fszero/` (pass-19 / pass-20).

Parent bead: `fszero-w2g.52`. Child pins: `fszero-w2g.29` (resolve),
`fszero-w2g.30` (telemetry). Speculative frames (`fszero-w2g.32`–`.34`) are
closed as non-product research metaphors, not shipped contracts.

## 1. What `fs.resolve` optimizes (and does not)

**Production ranking is a lexical / identifier heuristic**, not labeled
ground-truth relevance.

Implementation authority: `src/core/resolve.rs`.

| Signal | Role | Weight / behavior |
| --- | --- | --- |
| Symbol-ident match (`score_symbol_ident`) | Primary when a symbol hits | Multiplied (~3x) into path score |
| Path-segment / path-token match (`score_path_segments`) | Lexical path inflate | Always applied to path candidates |
| Line-text match | Content-line candidate score | Lexical only |
| Co-access affinity | Optional re-rank boost | `COACCESS_WEIGHT = 0.3` on normalized coaccess; multiplies existing score |

**Contract statements agents and optimizers must treat as true:**

1. Rank order is **best-effort discovery order** for human/agent navigation.
2. Score is **not** a calibrated P(relevant | intent) and must not be exported
   as quality, precision, or recall.
3. Co-access and path-token hits are **proxies**. Maximizing them alone can
   invert true intent relevance (pass-19 candidate `coaccess_pollution_goodhart`
   in `resolve_ranking_objective.json`).
4. Hard product constraints that *are* enforceable without labeled relevance:
   - returned count respects `limit`
   - when a symbol exists in scope, resolve must not return empty solely due
     to ranking proxy tuning

**What may never ship as a success KPI for resolve:**

- coaccess hit rate
- path_token hit rate
- raw score sum / mean score
- "top-1 relevance" without an external labeled set and a named denominator

Evidence packet:
`.math-review/runs/20260717T052725Z-fszero/passes/pass-19/evidence/resolve_ranking_objective.out.json`
(intentional Goodhart stress: optimization objective FAILs when proxy escapes).

## 2. Telemetry: `saved_tokens` is accounting, not quality

Authority: `src/core/telemetry.rs`, operator surface `docs/telemetry.md`.

| Field | Meaning |
| --- | --- |
| `raw_tokens` | Local aggregate estimate of tokens seen/accounted |
| `saved_tokens` | Local aggregate estimate of tokens not re-materialized (accounting delta) |
| `exporter` | Always `"none"` -- no upload path |
| CodeMode `measurement_coverage` | Per-execution measured/unmeasured honesty envelope |

**Contract:**

1. `saved_tokens` and related estimates are **bookkeeping**. They do not score
   answer correctness, tool choice quality, or session success.
2. Shareable inspect payload is allowlisted and default-off. Inflating counters
   must never change decision surfaces (dispatch, ranking, admission, gates).
3. Dashboards, bakeoffs, and agent self-scores must not treat
   `saved_tokens` (or `proxy_saved_token_rate`) as the sole or primary quality
   objective. Prefer labeled denominators (Q99-State/Input/Total when claims
   are generated from receipts) and task-outcome metrics.
4. `export_shareable_telemetry` returns `None` -- there is no network export
   that could be gamed for leaderboard upload.

Evidence packet:
`.math-review/runs/20260717T052725Z-fszero/passes/pass-19/evidence/telemetry_saved_tokens_objective*.json`
(candidate `inflate_saved_tokens_goodhart` dominated on honesty/quality).

## 3. Research frames vs product proof

Speculative metaphors from pass-20 (`light-cone`, `holographic API`,
`many-worlds merge`) are **research language only**. They are not product
contracts, not ABI, and not acceptance criteria. Product successors:

| Speculative frame | Product authority instead |
| --- | --- |
| Light-cone lease | Bounded deadline / resource envelopes |
| Holographic surface | Capability catalogs + conformance |
| Many-worlds merge | Certified overlay / publication contracts |

Deep concurrent MCP+world property fuzz remains an explicit coverage deferral
(`fszero-w2g.35`), not a silent "proved absent."

## 4. Operator checklist

Before closing an optimization or ranking change:

- [ ] Name the real objective metric (or pin "heuristic only").
- [ ] List proxy metrics that must **not** be maximized in isolation.
- [ ] Confirm no dashboard/KPI treats `saved_tokens` as quality.
- [ ] Keep speculative language out of user-facing success claims.
- [ ] Prefer receipt-generated claims with labeled denominators.

## Related docs

- `docs/telemetry.md` -- shareable vs local surfaces; KPI pin section
- `docs/design/search-prefilter-eval.md` -- hard FN=0 for prefilter throughput
- `docs/benchmark-integrity.md` -- measurement honesty for benches
