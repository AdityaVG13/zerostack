# Phase 12 remediation -- scored isomorphic rewrites + AUTO-FIX

**Date:** 2026-08-17
**HEAD intent:** `phase12: remediation plan scored`
**Mode:** design every remaining gap; AUTO-FIX only in-hub <~40-line leftovers

Full scored table: [REMEDIATION_PLAN.md](REMEDIATION_PLAN.md)

## AUTO-FIXES

| Item | Change |
|---|---|
| SURF-0010 / IDEA-0015 / ADV-0003 | `zero-mcp` tests: late Ok is `commit_race` / `retryable=false` / payload kept; late domain Err stays that Err; inflight released |
| SURF-0011 / IDEA-0012 | `zsx-core` empty Q99 window is `unavailable` + `no_demand_observations` + no bare `%` |
| SURF-0006 docs | `ci.yml` + `AGENTS-MANDATE.md` name rch as the test runner; no GH test job |
| OPEN-0014 | `dsr quality --tool zerostack` has zero checks → `NO_EVIDENCE` |

## Matrix

`F-CODEMODE-CANCEL` **present**. `F-ZSX-Q99-REPORT` stays **partial**. present=67 partial=7 missing=0 excluded=3. effective=0.952282 strict=0.940573 **gate=red**.

## Remaining CONFIRMED_GAP: 3

`SURF-0006` CI tests, `PERF-0001` `cv_pct` null, `PERF-0003` cass. OPEN=15.

## Do not

TokenZero Exact, engine ClampEnd, conformance CLI, rival-dirty tree, keep-gate wins, GH `cargo test --workspace`.
