# Phase 12 — keep-gate / bench-history stand-up

**Bead:** `zerostack-gauntlet-p12-keep-gate-perf-txll`  
Design pack. Do **not** `apply-ratchet.sh`. Do **not** seed `0.220207`.

## Rewrite A

Workspace-first keep-gate on existing benches. Emit `.bench-history/*.latest.json` with `cv_pct`. Reconstruct `ResourceGauge` inside each charge iter. Drop `NO_GLOBAL_DAEMON` as a keep predicate.

Full comprehensive-bench binary only after history exists.

## Do-not-ratchet

`reports/ratchet_state.json` stays `uninitialized: true` until three-pillar observations exist and `conformal_band.status=calibrated`.

cass 60-day mine remains BLOCKER (`p8-cass-blocker-9o0i`).
