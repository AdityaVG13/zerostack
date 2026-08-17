# Phase 9 -- baseline run (round 0)

**Date:** 2026-08-17T12:23:36Z
**Target HEAD (self-oracle pin):** `08c088e149b1bf06283b010ca838041fc3121d0f`
**Mode:** `gauntlet-greenfield`
**Class:** `Greenfield-Rust-class`
**Machine:** local `Adityas-MacBook-Pro-5.local`; cargo offloaded to rch worker `spark-1672`

This pass **ran** the existing gates. It did not add harnesses, did not
optimize, and did not invent bench numbers. Scorecard:
`docs/progress/baseline_round_0.json`.

`conformance/reports/` is gitignored engine evidence. Tracked artifacts
live here under `docs/progress/`.

## Headline numbers (truncate_score, 6 decimals)

| Metric | Value |
|---|---|
| FeatureUniverse | 77 features; present=61 partial=9 missing=4 excluded=3 |
| weight_sum | 1.000000 |
| effective_coverage (in-scope) | **0.910788** |
| strict_coverage (excluded is debt) | **0.899590** |
| dashboard release gate | **red** (honest; missing + excluded remain) |
| conformal / release decision | use **0.899590** lower/strict bound, not the 0.910788 point |
| bench-history | pass (self-oracle, no regression) |
| savings-bench primary_score | 0.044 (unchanged; lower_is_better token ratio) |
| cv_pct | **null** -- unknown; **not a win**; keep_eligible=false |
| zerostack-harness tests | 53 passed, 0 failed |
| zero-ref tests | 3 passed, 0 failed |
| store_cas_microbench | 2 passed (test, not a long bench) |
| oracle-preflight-doctor | **red**, certifying=**false** |
| TrueDivergence / FailureBundle | 0 / 0 |

## Gate results (actually run)

### Python (local)

```
python3 scripts/check_feature_universe_weights.py
# feature-universe ok: features=77 present=61 partial=9 missing=4 excluded=3 weight_sum=1.000000000000
# EXIT=0

python3 scripts/check_feature_coverage_dashboard.py
# feature-coverage-dashboard ok: ... effective=0.910788 strict=0.899590 gate=red families=21 catalog=6
# EXIT=0

python3 scripts/check_golden_integrity.py
# golden-integrity ok: checksums=12 artifacts=11 tier1=5 schema=1.0.0
# EXIT=0

python3 scripts/check_bench_history.py --self-test
# bench-history self-test ok
# EXIT=0

python3 scripts/check_bench_history.py
# primary_score: unchanged
# geomean / p90 / throughput: absent; not invented, not gated
# category:exact_tokens: unchanged
# cv_pct is null (unknown; not a win)
# verdict: pass (no regression). within-noise / unknown-cv is not a win
# EXIT=0

python3 scripts/check_ledger_retry.py
# ledger-retry ok: files=3
# EXIT=0
```

### Cargo via rch (`spark-1672`, `CARGO_TARGET_DIR=/tmp/rch_target_zerostack`)

```
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  cargo test -p zerostack-harness -- --test-threads=1
# lib unit 31; crash_oracle 5; golden_invariants 6; oracle_smoke 9;
# store_cas_microbench 2; bin 0; doctests 0
# test result: ok. 53 passed; 0 failed
# EXIT=0

rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  cargo test -p zero-ref -- --test-threads=1
# lib unit 0 (autotests=false); zeroref_proptest 2; doctest 1
# test result: ok. 3 passed; 0 failed
# EXIT=0

rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json
# aggregate_outcome=red certifying=false EXIT=1
# first_failure_diagnosis:
#   spec_source_sha256: agents-law (AGENTS.md) sha256 drifted
#   expected da53da8383380c712f781c5f5cc85f48e56bc43281f824af11e47fabb7cca6f4
#   got      7211bb0e78819cf5b6405219a209329c5c12616ab8eff5f09cd1128271b4c5b2
```

`store_cas` is `crates/zerostack-harness/tests/store_cas_microbench.rs`.
It ran as part of the harness crate. It is a test, not a long bench.

Never `cargo test --workspace`.

## Failures found

### 1. Preflight agents-law hash drift -- NOT fixed

`AGENTS.md` is gitignored. Phase 8 appended the negative-evidence
mandate, so the working-copy hash moved from the Phase 3/4 pin
(`da53da83…`) to `7211bb0e…` (same value locally and on spark-1672).

`cargo test -p zerostack-harness` still passed:
`oracle_smoke::preflight_does_not_panic_and_is_green` does **not**
require aggregate green. It only requires a diagnosis when red, and
`certifying == (aggregate == green)`.

Not fixed in this pass. Re-pinning the contract hash would require an
operator bless of `conformance/contracts/spec_version_contract.toml`
and its Tier1Raw golden (`check_golden_integrity.py` never blesses).
Phase 9 does not rewrite goldens.

Not a `TrueDivergence`. No FailureBundle.

### Cargo tests

None failed. No harness/script test bug to patch.

## Dashboard (gate red is honest)

Weight policy: global-sum-1.0. Partial never rounds up. Excluded is
strict-100% debt: `strict_coverage` (0.899590) < `effective_coverage`
(0.910788). `strict_100_certifiable: false`.

| Family | present | partial | missing | excluded | verdict | weighted | strict |
|---|---:|---:|---:|---:|---|---:|---:|
| ci | 0 | 1 | 0 | 0 | partial | 0.500000 | 0.500000 |
| conformance | 1 | 1 | 0 | 0 | partial | 0.687499 | 0.687499 |
| engine-scope | 0 | 0 | 0 | 3 | none | 0.000000 | 0.000000 |
| gauntlet-oracle | 2 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| hub-policy | 3 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| machine-permit | 2 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| quality | 0 | 0 | 2 | 0 | none | 0.000000 | 0.000000 |
| surface | 1 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-abi | 10 | 1 | 0 | 0 | partial | 0.941176 | 0.941176 |
| zero-cert | 2 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-codemode | 2 | 1 | 0 | 0 | partial | 0.850000 | 0.850000 |
| zero-gate | 3 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-gauge | 1 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-ledger | 2 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-mcp | 2 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-process | 2 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zero-ref | 5 | 2 | 2 | 0 | partial | 0.828947 | 0.828947 |
| zero-store | 12 | 2 | 0 | 0 | partial | 0.979166 | 0.979166 |
| zsx | 3 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| zsx-core | 7 | 1 | 0 | 0 | partial | 0.961538 | 0.961538 |
| zsx-node | 1 | 0 | 0 | 0 | full | 1.000000 | 1.000000 |
| **global** | **61** | **9** | **4** | **3** | **red** | **0.910788** | **0.899590** |

Still missing (unchanged): `F-FUZZ`, `F-MIRI-NARROW`,
`F-REF-SERDE-FROMSTR`, `F-REF-CAPABILITY-NEGOTIATION`.

Still excluded (engine-only): `F-FSZERO-PRIVATE-ENGINE-SURFACE`,
`F-GRAPHZERO-PRIVATE-ENGINE-SURFACE`,
`F-TOKENZERO-PRIVATE-ENGINE-SURFACE`.

## Self-oracle pin

`self_oracle_prior_commit_sha = 08c088e149b1bf06283b010ca838041fc3121d0f`

That is target HEAD **before** any Phase 9 scorecard commit. Subject is
the current hub. The savings-bench seed remains
`.bench-history/savings-bench.latest.json` (`seed_kind =
self-oracle-prior-commit`, `cv_pct = null`, `keep_eligible = false`,
`git_sha` in that seed is still null -- the v1 run did not record it;
this pin lives on the Phase 9 scorecard, not as a rewritten historical
environment).

## Already correct (left alone)

1. FeatureUniverse `sum(weights) == 1.0` and required retry_condition on partial/missing/excluded
2. Dashboard gate **red** with `partial_never_rounds_up` and excluded-as-debt
3. Three-tier goldens + `check_golden_integrity.py` (Tier-2 never labeled Tier-1)
4. Bench-history ratchet (`cv_pct` null is not a win; no faster-than-reference claim)
5. Ledger retry-condition lint (three files, eight vocabulary forms)
6. EngineIdentity Subject≠Oracle + both-error comparator
7. Crash oracle + FaultPlan named boundaries (5 tests green)
8. FailureBundle schema (`/failure/first_divergence`) -- unused this round because no TrueDivergence
9. `store_cas_microbench` emits JSON v3 as a test, not a keep
10. CONTRACT §8: no in-repo conformance CLI (`F-CONF-HARNESS` stays partial)

## What this pass did not do

No comprehensive-bench 93-scenario matrix. No flamegraph / samply / dhat
/ strace. No fuzz. No miri. No goldens bless. No product-hot-path
change. Rival dirty files were not touched:
`crates/zsx-core/src/fszero.rs`, `docs/codemode.md`,
`tests/unit/zsx-core/fszero_tests.rs`, `.zsx_patch.diff`.
