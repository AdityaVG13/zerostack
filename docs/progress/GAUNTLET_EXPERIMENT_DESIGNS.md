# Gauntlet experiment designs -- Phase 10 (round 0)

**Date:** 2026-08-17
**Mode:** `gauntlet-greenfield`
**Class:** `Greenfield-Rust-class`
**Target HEAD at design time:** `7f1567932dafc86665fc6b23825cbf35174ef73d`
**Self-oracle pin (Phase 9):** `08c088e149b1bf06283b010ca838041fc3121d0f`
**Created by:** idea-wizard-orchestrator + advanced-methods-miner (Phase 10)
**Do not implement remediations in this file.** Pass 11 consumes `CONFIRMED_GAP` and `OPEN`.

Template fields per card: hypothesis / motivation / minimal reproducer / expected signal / falsifiability / one-line invocation / results inline / closure predicate. Status is exactly one of `OPEN | CONFIRMED_GAP | NO_EVIDENCE | NEEDS_REFINEMENT | NEW_HYPOTHESIS_SPAWNED | CLOSED`.

`CONFIRMED_GAP` is used only when the Phase 9 baseline, FeatureUniverse matrix, or a cited source file already proves the absence. Already-implemented harness pieces are `CLOSED` with commit SHAs so the open-hypothesis count stays honest.

Pillar indexes:

- [PERF_HYPOTHESIS_LEDGER.md](PERF_HYPOTHESIS_LEDGER.md)
- [CONFORMANCE_HYPOTHESIS_LEDGER.md](CONFORMANCE_HYPOTHESIS_LEDGER.md)
- [SURFACE_PARITY_HYPOTHESIS_LEDGER.md](SURFACE_PARITY_HYPOTHESIS_LEDGER.md)

Negative-evidence ledgers (grep first): `perf-negative-results.md`, `conformance-negative-results.md`, `surface-deferrals.md`.

---

## Census

| Status | Count | Meaning for pass 11 |
|---|---:|---|
| `CLOSED` | 25 | Already correct, or pass-11 remediations landed. |
| `CONFIRMED_GAP` | 5 | Remaining holes (CI tests, cancel hub test, Q99 residual, plus non-ranked). |
| `OPEN` | 19 | Experiment not yet run, or unverified SPEC tag. Do not treat as a bug. |
| `NO_EVIDENCE` | 0 | -- |
| `NEEDS_REFINEMENT` | 0 | -- |
| `NEW_HYPOTHESIS_SPAWNED` | 0 | Spawned children are listed as their own `OPEN`/`CONFIRMED_GAP` cards. |
| **Total cards** | **49** | |
| **Open-hypothesis count** | **19** | `OPEN` only. `CONFIRMED_GAP` is resolved-as-gap. |

---

## Ranked pass-11 implementability (in-hub, smallest-correct)

Do these first. Each is hub-local, small, and already a `CONFIRMED_GAP`.

| Rank | ID | Gap | Why first | Out of scope? |
|---:|---|---|---|---|
| 1 | SURF-0003 | `F-REF-SERDE-FROMSTR` | `impl FromStr for ZeroRefV1` + serde of Display form. `zero-ref` already depends on `serde`. One unit test. | no |
| 2 | CONF-0006 | Preflight `AGENTS.md` hash drift | Re-pin `spec_version_contract.toml` agents-law sha256 to `7211bb0e…` and bless the Tier1Raw golden. Config, not product. | no (operator bless) |
| 3 | SURF-0001 | `F-FUZZ` | One `cargo-fuzz` target over the checked-in untrusted-bytes corpus. Floor, not a campaign. | no |
| 4 | SURF-0007 | `F-ABI-PROPTEST-ROUNDTRIP` | `proptest!` encode/decode identity on a `zero-abi` wire type. No tests dir today. | no |
| 5 | SURF-0009 | `F-STORE-ENSURE-LAYOUT` | `ensure_layout` creates `unified_root` + `engine_dir` only. Either create `blobs/` + `gc/` or return a loud typed error; unit test the chosen contract. | no |
| 6 | SURF-0004 | `F-REF-CAPABILITY-NEGOTIATION` | Public `negotiate(major, minor) -> Result<(), ZeroRefError>` returning `IncompatibleVersion` on major mismatch. Consts already exist. | no |
| 7 | SURF-0012 | `F-REF-ERROR-TAXONOMY` | Table test that `ZeroRefErrorClass::ALL` is constructed or explicitly out-of-parser. Five variants never built in `lib.rs`. | no |
| 8 | SURF-0002 | `F-MIRI-NARROW` | DSR/rch job: `cargo +nightly miri test -p zero-ref`. Crate is `#![forbid(unsafe_code)]`. Not a product patch. | no (CI/DSR) |

**Do not implement in-hub (out-of-repo or decided):**

| ID | Why pass 11 must not "fix" it in this repo |
|---|---|
| SURF-0005 `F-CONF-HARNESS` | CONTRACT.md §8 forbids an in-repo conformance CLI. Already `CLOSED`. |
| SURF-0008 `F-REF-ENGINE-ADOPTION-LOCKSTEP` | Hub cannot enforce sibling engines. Out-of-repo. |
| SURF-0014 engine-only surfaces | FSZero / GraphZero / TokenZero are not workspace members. |
| SURF-0011 `F-ZSX-Q99-REPORT` residual | Empty L1/L2 needs engine `WorkerTokenAccountingV1`. Hub can only test the loud-unavailable path. |
| CONF-0003 `SPEC-HON-006` | Permission for TokenZero Exact. Engine-owned. |
| PERF-0002 fat-LTO | Named waiver vs `release-node`. Already `CLOSED`. |
| SURF-0015 dashboard red | Gate is honest. Closing it means closing the missing/partial rows, not flipping the gate. |

---

## Already correct (CLOSED -- do not re-open)

These landed in passes 3--9. Evidence is the cited SHA plus a green Phase 9 gate.

| What | SHA | Card |
|---|---|---|
| EngineIdentity Subject≠Oracle | `7992967ee853d5f17ed98d0cbb32cdb4c2311d6f` | CLOSED-0001 |
| Both-error = agreement | `7992967ee853d5f17ed98d0cbb32cdb4c2311d6f` | CLOSED-0002 |
| FailureBundle `/failure/first_divergence` | `9081c2c721c8661bfbfd47693c37ee3e66e1d830` | CLOSED-0003 |
| Crash oracle + FaultPlan | `9081c2c721c8661bfbfd47693c37ee3e66e1d830` | CLOSED-0004 |
| `zero-ref` proptest parse/Display | `9081c2c721c8661bfbfd47693c37ee3e66e1d830` | CLOSED-0005 |
| Bench-history ratchet | `d6cf7ed22618f9b6289745c822f935c3eb59b828` | CLOSED-0006 |
| `release-perf` line tables + fat-LTO waiver | `d6cf7ed22618f9b6289745c822f935c3eb59b828` | PERF-0002 |
| FeatureUniverse + dashboard (gate red is honest) | `d14141342637ec8b7de6b9208b633740f9995d7e` | CLOSED-0007, SURF-0015 |
| Three-tier goldens + integrity | `738a803975a40b3666bb482e4bb70da283af5df2` | CLOSED-0008 |
| `sum(weights) == 1.0` global | `cf6ca4da1776a9180efc6a745b52ac69087bc7aa` | CLOSED-0009 |
| CONTRACT §8: no in-repo conformance CLI | `9081c2c721c8661bfbfd47693c37ee3e66e1d830` | SURF-0005 |
| Ledger retry-condition lint | `08c088e149b1bf06283b010ca838041fc3121d0f` | CLOSED-0010 |
| Test-profile microbench is not a keep | `d6cf7ed22618f9b6289745c822f935c3eb59b828` | CLOSED-0011 |
| Engine-only surfaces excluded-as-debt | `d14141342637ec8b7de6b9208b633740f9995d7e` | SURF-0014 |
| Engine ClampEnd lockstep cannot be hub-enforced | matrix + Phase 9 | SURF-0008 |
| `store_cas_microbench` JSON v3 as a test | Phase 9: 2 passed | CLOSED-0011 |

---

## CLOSED-0001 -- EngineIdentity Subject≠Oracle

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0001` |
| `pillar` | `conformance` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |
| `status` | `CLOSED` |

### Hypothesis

> The harness comparator can be entered with Subject label equal to an Oracle label, producing a fake 100% pass rate.

### Motivation

Greenfield anti-pattern: oracle compared against itself. Phase 3 wired a distinct `EngineIdentity` type (not `raw_worker::EngineIdentity`).

### Minimal Reproducer

```bash
rg -n "SUBJECT_IDENTITY_LABEL|fn assert_distinct|Subject==Oracle" crates/zerostack-harness/src/engine_identity.rs crates/zerostack-harness/src/oracle.rs
```

### Expected Signal

If the hypothesis were true: preflight `engine_identity_distinct` red, or comparator accepts `zerostack` vs `zerostack`.

### Falsifiability Criteria

Subject label is `zerostack` and every oracle label is in `{spec-v1, property-suite-v1, prior-commit-<sha>, round-trip, miri, clippy}`; comparator panics or returns hard failure on equal labels.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --lib engine_identity -- --test-threads=1
```

### Results Inline

```
result_status: CLOSED
result_summary: Implemented in pass 3. Phase 9 preflight check engine_identity_distinct=green. F-ORACLE-ENGINE-IDENTITY present.
result_evidence_paths:
  - crates/zerostack-harness/src/engine_identity.rs
  - docs/progress/baseline_round_0.json
  - docs/progress/conformance-negative-results.md (self-compare-oracle-identity CLOSED)
result_impact: structural; no TrueDivergence this round
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

### Cross-References

- Operator: `🪞` Engine-Identity-Guard
- Related ledger: `self-compare-oracle-identity`
- SHA: `7992967ee853d5f17ed98d0cbb32cdb4c2311d6f`

---

## CLOSED-0002 -- both-error is agreement

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0002` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `(Err(_), Err(_))` pairs with different messages are scored as TrueDivergence.

### Motivation

Agreement-by-error-message-string is a named anti-pattern. Phase 3/6 wired both-error = agreement regardless of text.

### Minimal Reproducer

```bash
rg -n "both.error|BothError|one-error" crates/zerostack-harness/src/oracle.rs
```

### Expected Signal

If true: a dual-error scenario with distinct strings fails the comparator.

### Falsifiability Criteria

`INV-BOTH-ERROR` enforced; one-error-one-OK remains a hard failure.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --lib oracle -- --test-threads=1
```

### Results Inline

```
result_status: CLOSED
result_summary: Rejected as a product change; comparator already treats both-error as agreement. Ledger both-error-as-failure REJECTED.
result_evidence_paths:
  - crates/zerostack-harness/src/oracle.rs
  - conformance/contracts/invariant_catalog.toml
result_impact: none
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Not worth retrying as a standalone patch.

---

## CLOSED-0003 -- FailureBundle schema

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0003` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Divergences emit no `/failure/first_divergence` pointer, so Phase 9 cannot attribute.

### Motivation

Polish bar: every E2E failure emits FailureBundle v1.0.0. Phase 6 landed the type.

### Minimal Reproducer

```bash
rg -n "FIRST_DIVERGENCE_JSONPTR|first_divergence" crates/zerostack-harness/src/failure_bundle.rs
```

### Expected Signal

If true: no `FailureBundle` type, or pointer field absent.

### Falsifiability Criteria

`SCHEMA_VERSION = failure_bundle.v1.0.0` and `FIRST_DIVERGENCE_JSONPTR = /failure/first_divergence`. Phase 9 emitted 0 bundles because TrueDivergence count was 0, not because the type is missing.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --lib failure_bundle -- --test-threads=1
```

### Results Inline

```
result_status: CLOSED
result_summary: Present. F-ORACLE-FAILURE-BUNDLE present. Phase 9 failure_bundles_emitted=0 with true_divergences=[].
result_evidence_paths:
  - crates/zerostack-harness/src/failure_bundle.rs
  - docs/progress/baseline_round_0.json
result_impact: unused this round because no TrueDivergence
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

### Cross-References

- SHA: `9081c2c721c8661bfbfd47693c37ee3e66e1d830`

---

## CLOSED-0004 -- crash oracle

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0004` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Named journal crash boundaries are unwired; MidJournalRecover is untested.

### Motivation

Phase 6 crash-oracle suite. Phase 9 ran 5 crash_oracle tests, all green.

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --test crash_oracle -- --test-threads=1
```

### Expected Signal

If true: test binary missing or failures at AfterTmpWriteBeforeRename / MidJournalRecover.

### Falsifiability Criteria

5 passed, 0 failed (Phase 9 scorecard). `F-STORE-CRASH-ORACLE` present.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --test crash_oracle -- --test-threads=1
```

### Results Inline

```
result_status: CLOSED
result_summary: 5 crash_oracle tests green at Phase 9. Feature present.
result_evidence_paths:
  - crates/zerostack-harness/tests/crash_oracle.rs
  - docs/progress/baseline_round_0.json
result_impact: 5/5 pass
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

### Cross-References

- SHA: `9081c2c721c8661bfbfd47693c37ee3e66e1d830`

---

## CLOSED-0005 -- zero-ref proptest

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0005` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> ZeroRef parse/Display has no property suite.

### Motivation

`F-REF-PROPTEST` is **present**. Distinct from missing `F-ABI-PROPTEST-ROUNDTRIP`.

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-ref --test zeroref_proptest -- --test-threads=1
```

### Expected Signal

If true: no `crates/zero-ref/tests/zeroref_proptest.rs`.

### Falsifiability Criteria

File exists; Phase 9 `zero-ref` tests 3 passed (2 proptest + 1 doctest).

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-ref --test zeroref_proptest -- --test-threads=1
```

### Results Inline

```
result_status: CLOSED
result_summary: present. Do not confuse with SURF-0007 (zero-abi has no proptest).
result_evidence_paths:
  - crates/zero-ref/tests/zeroref_proptest.rs
  - docs/progress/baseline_round_0.json
result_impact: 2 proptest tests green
spawned_remediation_bead: N/A
spawned_experiments: [SURF-0007]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

---

## CLOSED-0006 -- bench-history ratchet

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0006` |
| `pillar` | `perf` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> There is no committed `.bench-history` file, so pass-over-pass cannot run.

### Motivation

Phase 5 seeded savings-bench. Phase 9 `check_bench_history.py` exit 0 (no regression; not a win).

### Minimal Reproducer

```bash
python3 scripts/check_bench_history.py
```

### Expected Signal

If true: script missing or `.bench-history/savings-bench.latest.json` absent.

### Falsifiability Criteria

File committed; `F-STORE-BENCH-HISTORY` present; `cv_pct` null is recorded, not invented. See PERF-0001 for the keep-eligibility gap.

### One-Line Invocation

```bash
python3 scripts/check_bench_history.py && python3 scripts/check_bench_history.py --self-test
```

### Results Inline

```
result_status: CLOSED
result_summary: Ratchet exists and is honest. Seed is not a keep (PERF-0001).
result_evidence_paths:
  - .bench-history/savings-bench.latest.json
  - scripts/check_bench_history.py
result_impact: verdict=pass; keep_eligible=false
spawned_remediation_bead: N/A
spawned_experiments: [PERF-0001]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

### Cross-References

- SHA: `d6cf7ed22618f9b6289745c822f935c3eb59b828`

---

## CLOSED-0007 -- FeatureUniverse + dashboard loader

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0007` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> FeatureUniverse weights do not sum to 1.0, or the dashboard rounds partial up to green.

### Motivation

Pass 2/7. `F-FEATURE-UNIVERSE-INREPO` present. Gate red is a separate honesty card (SURF-0015).

### Minimal Reproducer

```bash
python3 scripts/check_feature_universe_weights.py && python3 scripts/check_feature_coverage_dashboard.py
```

### Expected Signal

If true: weight_sum != 1.0 or gate=green with missing rows.

### Falsifiability Criteria

weight_sum=1.000000; effective=0.910788; strict=0.899590; gate=red; 77 features.

### One-Line Invocation

```bash
python3 scripts/check_feature_universe_weights.py && python3 scripts/check_feature_coverage_dashboard.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Loader and dashboard are correct. Remaining holes are the missing/partial rows, not the dashboard.
result_evidence_paths:
  - conformance/contracts/supported_surface_matrix.toml
  - docs/progress/baseline_round_0.json
result_impact: gate=red is the honest output
spawned_remediation_bead: N/A
spawned_experiments: [SURF-0015]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

### Cross-References

- SHA: `d14141342637ec8b7de6b9208b633740f9995d7e`

---

## CLOSED-0008 -- three-tier goldens

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0008` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Golden artifacts lack a tier label or checksum guard, so Phase 9 can bless a Tier-2 match as Tier-1.

### Motivation

Phase 4. `F-CONF-GOLDEN-SCHEMA` present.

### Minimal Reproducer

```bash
python3 scripts/check_golden_integrity.py
```

### Expected Signal

If true: script red or schema != 1.0.0.

### Falsifiability Criteria

Phase 9: checksums=12 artifacts=11 tier1=5 schema=1.0.0 EXIT=0.

### One-Line Invocation

```bash
python3 scripts/check_golden_integrity.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Integrity gate green. Phase 9 did not bless goldens (AGENTS hash still red).
result_evidence_paths:
  - conformance/golden/
  - docs/progress/baseline_round_0.json
result_impact: none
spawned_remediation_bead: N/A
spawned_experiments: [CONF-0006]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

### Cross-References

- SHA: `738a803975a40b3666bb482e4bb70da283af5df2`

---

## CLOSED-0009 -- global weight sum 1.0 waiver

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0009` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> In-repo global `sum(weights) == 1.0` (no per-family 1.0) is a FeatureUniverse bug.

### Motivation

Surface-deferrals `global-sum-weight-waiver` CLOSED. Hub is one product universe. `[SPEC-FU-003]`.

### Minimal Reproducer

```bash
python3 scripts/check_feature_universe_weights.py
```

### Expected Signal

If true: script requires per-family 1.0 and fails.

### Falsifiability Criteria

`weight_policy = "global-sum-1.0"` is documented and enforced.

### One-Line Invocation

```bash
python3 scripts/check_feature_universe_weights.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Waiver, not a bug. Do not rebalance to per-family 1.0.
result_evidence_paths:
  - docs/progress/surface-deferrals.md
  - conformance/contracts/supported_surface_matrix.toml
result_impact: none
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Reconsider only inside the broader parity-score category redesign (track as SPEC-FU-003).

---

## CLOSED-0010 -- ledger retry lint

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0010` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Negative ledgers accept "later" / "TBD" retry phrases.

### Motivation

Phase 8. Three files, eight vocabulary forms.

### Minimal Reproducer

```bash
python3 scripts/check_ledger_retry.py
```

### Expected Signal

If true: script missing or EXIT!=0.

### Falsifiability Criteria

Phase 9: ledger-retry ok files=3 EXIT=0.

### One-Line Invocation

```bash
python3 scripts/check_ledger_retry.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Lint green. Hypothesis ledgers in this file are not linted by that script.
result_evidence_paths:
  - scripts/check_ledger_retry.py
  - docs/progress/perf-negative-results.md
result_impact: none
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

---

## CLOSED-0011 -- test-profile microbench is not a keep

| Field | Value |
|---|---|
| `experiment_id` | `CLOSED-0011` |
| `pillar` | `perf` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `store_cas_microbench` under the test profile is keep-eligible evidence for a product hot-path change.

### Motivation

Rejected in `perf-negative-results.md` (`test-profile-microbench-as-keep`). Phase 9 ran it as a test (2 passed).

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --test store_cas_microbench -- --test-threads=1
```

### Expected Signal

If the rejected hypothesis were adopted: a keep would be claimed from test-profile JSON.

### Falsifiability Criteria

Emitted JSON has `keep_eligible: false`. Keep-gate requires `release-perf` + cv_pct + profile-first.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --test store_cas_microbench -- --test-threads=1
```

### Results Inline

```
result_status: CLOSED
result_summary: Test exists; keep is forbidden. Spawn PERF-0007 for a real release-perf profile.
result_evidence_paths:
  - crates/zerostack-harness/tests/store_cas_microbench.rs
  - docs/progress/perf-negative-results.md
result_impact: 2 passed; not a keep
spawned_remediation_bead: N/A
spawned_experiments: [PERF-0007]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry only if a profiler attributes a clearly-above-noise share to SharedCas::put or atomic_write_file on a release-perf store_cas workload with cv_pct at or below 5.

---

## SURF-0001 -- F-FUZZ missing

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0001` |
| `pillar` | `surface` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-FUZZ` |
| `parent_hypothesis_id` | `N/A` |
| `status` | `CLOSED` |

### Hypothesis

> The hub has no `fuzz/fuzz_targets/` cargo-fuzz campaign; the checked-in untrusted-bytes corpus file is not a fuzz target.

### Motivation

Matrix `F-FUZZ` status=`missing`. Phase 7/9: still missing. Family `quality` verdict `none`. Surface-deferrals `F-FUZZ` DEFERRED.

### Minimal Reproducer

```bash
test ! -d fuzz/fuzz_targets && echo 'no fuzz dir' ; cargo fuzz list 2>&1 | head
```

### Expected Signal

`fuzz/` is absent (CodeMode `fs.ls fuzz` = not_found). `cargo fuzz list` does not name a ZeroRef/CAS/raw_worker target.

### Falsifiability Criteria

If `fuzz/fuzz_targets/` contains a target and `cargo fuzz list` names it, the hypothesis is refuted and the matrix row must move to `present`.

### One-Line Invocation

```bash
python3 -c "import os,sys; sys.exit(0 if not os.path.isdir('fuzz/fuzz_targets') else 1)" ; cargo fuzz list
```

### Results Inline

```
result_status: CLOSED
result_summary: cargo-fuzz workspace at fuzz/; targets zeroref_parse and abi_frame_decode. F-FUZZ present. Floor, not a 24h campaign.
result_evidence_paths:
  - fuzz/fuzz_targets/zeroref_parse.rs
  - fuzz/fuzz_targets/abi_frame_decode.rs
  - conformance/contracts/supported_surface_matrix.toml (F-FUZZ)
result_impact: quality family no longer none
spawned_remediation_bead: F-FUZZ
spawned_experiments: [IDEA-0003]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed when fuzz/fuzz_targets/ contains at least one cargo-fuzz target for ZeroRef parse or raw_worker frame decode and cargo fuzz list names it.

### Cross-References

- Operator: `🧪` Experiment-Design
- Related ledger: `F-FUZZ` in `surface-deferrals.md`

---

## SURF-0002 -- F-MIRI-NARROW missing

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0002` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-MIRI-NARROW` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> No CI or rch/DSR job runs `cargo +nightly miri test -p zero-ref` (or zero-store hot paths); host toolchain presence is not the feature.

### Motivation

Matrix `F-MIRI-NARROW` missing. External-tool oracle is a version probe only (Phase 3). `.github/workflows/ci.yml` has no miri job.

### Minimal Reproducer

```bash
rg -n "miri" .github/workflows/ci.yml rust-toolchain.toml scripts || true
```

### Expected Signal

No workflow step invokes miri. Preflight oracle list includes `miri` as a version probe, not a test run.

### Falsifiability Criteria

A DSR or rch log showing `cargo +nightly miri test -p zero-ref` exit 0, wired so a UB report fails the gate.

### One-Line Invocation

```bash
rg -n "miri test" .github/workflows scripts || echo 'no miri test job'
```

### Results Inline

```
result_status: CLOSED
result_summary: scripts/run_miri_narrow.sh landed. Feature stays partial until miri test -p zero-ref is green on rch. Host rust-toolchain is still not the feature.
result_evidence_paths:
  - scripts/run_miri_narrow.sh
  - conformance/contracts/supported_surface_matrix.toml (F-MIRI-NARROW)
result_impact: quality family partial
spawned_remediation_bead: F-MIRI-NARROW
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Script exists. Remaining retry: miri test -p zero-ref green on rch.

---

## SURF-0003 -- F-REF-SERDE-FROMSTR missing

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0003` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-REF-SERDE-FROMSTR` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `ZeroRefV1` has `Display` and `parse()` but no `FromStr` and no serde of the Display form.

### Motivation

Matrix missing. `ZeroRefV1` derive is `Clone, Debug, PartialEq, Eq` only (`lib.rs` ~200). Serde is used on `ObjectId` / span types, not on the Display string.

### Minimal Reproducer

```rust
// must fail to compile today
let _ = "fz://blob/<64hex>".parse::<zero_ref::ZeroRefV1>();
```

### Expected Signal

No `impl FromStr for ZeroRefV1`. No `#[derive(Serialize, Deserialize)]` that round-trips the Display string.

### Falsifiability Criteria

`s.parse::<ZeroRefV1>()?` equals `ZeroRefV1::parse(s)?` in a unit test, and serde of the Display form agrees.

### One-Line Invocation

```bash
rg -n "impl FromStr for ZeroRefV1|struct ZeroRefV1" crates/zero-ref/src/lib.rs
```

### Results Inline

```
result_status: CLOSED
result_summary: FromStr delegates to parse(); serde (de)serializes the Display string. Unit tests in zeroref_api.rs.
result_evidence_paths:
  - crates/zero-ref/src/lib.rs
  - crates/zero-ref/tests/zeroref_api.rs
  - conformance/contracts/supported_surface_matrix.toml (F-REF-SERDE-FROMSTR)
result_impact: F-REF-SERDE-FROMSTR present
spawned_remediation_bead: F-REF-SERDE-FROMSTR
spawned_experiments: [IDEA-0001]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed when ZeroRefV1 implements FromStr and serde of the Display form with a unit test that parse-via-FromStr equals ZeroRefV1::parse.

---

## SURF-0004 -- F-REF-CAPABILITY-NEGOTIATION missing

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0004` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-REF-CAPABILITY-NEGOTIATION` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `ZEROREF_VERSION` / `MAJOR` / `MINOR` consts are not a `negotiate` API; peers cannot be refused before payload work.

### Motivation

Matrix missing. `IncompatibleVersion` exists as an error class but is never constructed in `crates/zero-ref/src/lib.rs`. Distinct from present `F-ABI-SHARED-CAPABILITY`.

### Minimal Reproducer

```bash
rg -n "fn negotiate|ZEROREF_MAJOR" crates/zero-ref/src/lib.rs
```

### Expected Signal

No public function accepts `(major, minor)` and returns `IncompatibleVersion` on major mismatch.

### Falsifiability Criteria

A public `negotiate` (name may vary) returns `IncompatibleVersion` for a different major before any payload work, covered by a unit test.

### One-Line Invocation

```bash
rg -n "pub fn negotiate" crates/zero-ref/src/lib.rs || echo 'no negotiate'
```

### Results Inline

```
result_status: CLOSED
result_summary: public negotiate(major, minor) returns IncompatibleVersion on major mismatch. Minor is additive.
result_evidence_paths:
  - crates/zero-ref/src/lib.rs
  - crates/zero-ref/tests/zeroref_api.rs
  - conformance/contracts/supported_surface_matrix.toml
result_impact: F-REF-CAPABILITY-NEGOTIATION present
spawned_remediation_bead: F-REF-CAPABILITY-NEGOTIATION
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed when a public function accepts a peer (major, minor) and returns IncompatibleVersion for a different major before any payload work.

---

## SURF-0005 -- F-CONF-HARNESS stays partial (CONTRACT §8)

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0005` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-CONF-HARNESS` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Promoting `F-CONF-HARNESS` to present requires an in-repo conformance CLI / second MCP catalog.

### Motivation

`invent-second-conformance-catalog` REJECTED. `[SPEC-NEG-001]`, `[SPEC-NEG-002]`. Harness crate exists; §8 still forbids a product CLI.

### Minimal Reproducer

```bash
rg -n "What is not claimed|conformance CLI" conformance/CONTRACT.md
```

### Expected Signal

If someone marks the row `present` without a §8 amendment, the FeatureUniverse overclaims.

### Falsifiability Criteria

CONTRACT.md §8 is amended to allow an in-repo conformance CLI. Until then the row stays `partial`.

### One-Line Invocation

```bash
python3 scripts/check_feature_universe_weights.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Partial is the honest status. Do not invent a CLI in pass 11.
result_evidence_paths:
  - conformance/CONTRACT.md
  - docs/progress/conformance-negative-results.md
result_impact: conformance family weighted 0.687499
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Reconsider only inside the broader CONTRACT.md §8 redesign (track as F-CONF-HARNESS).

---

## SURF-0006 -- F-CI-PR-GATES no cargo test job

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0006` |
| `pillar` | `surface` |
| `status` | `CONFIRMED_GAP` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-CI-PR-GATES` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `.github/workflows/ci.yml` is `workflow_dispatch` only and has no `cargo test` job. Manual fmt/clippy/build is not full PR CI.

### Motivation

Matrix partial. AGENTS.md says DSR is the real CI (GitHub Actions budget). The GH hole is proven; whether DSR already runs tests is OPEN-0014.

### Minimal Reproducer

```bash
rg -n "cargo test|on:" .github/workflows/ci.yml
```

### Expected Signal

`on: workflow_dispatch` only. Jobs: feature-universe, lint, build, privacy. No test job.

### Falsifiability Criteria

`ci.yml` or DSR runs `cargo test` (targeted or workspace) on every PR/push.

### One-Line Invocation

```bash
rg -n "cargo test|^on:" .github/workflows/ci.yml
```

### Results Inline

```
result_status: CONFIRMED_GAP
result_summary: GH workflow has no test job and no push/PR trigger. Do not spend GH Actions budget if DSR already gates tests -- check OPEN-0014 first.
result_evidence_paths:
  - .github/workflows/ci.yml
  - AGENTS.md (DSR section)
result_impact: ci family verdict=partial 0.500000
spawned_remediation_bead: F-CI-PR-GATES
spawned_experiments: [OPEN-0014]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Worth reconsidering when .github/workflows/ci.yml or DSR runs cargo test on every PR/push, not only workflow_dispatch fmt/clippy/build.

---

## SURF-0007 -- F-ABI-PROPTEST-ROUNDTRIP partial

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0007` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-ABI-PROPTEST-ROUNDTRIP` |
| `parent_hypothesis_id` | `CLOSED-0005` |

### Hypothesis

> `zero-abi` has types and digest but no `proptest!` encode/decode identity; `crates/zero-abi/tests/` does not exist.

### Motivation

Matrix partial. Distinct from present `F-REF-PROPTEST`. `zero-abi` Cargo.toml has no proptest dev-dependency.

### Minimal Reproducer

```bash
test ! -d crates/zero-abi/tests && echo 'no zero-abi tests dir'
rg -n "proptest!" crates/zero-abi || echo 'no proptest in zero-abi'
```

### Expected Signal

No `proptest!` in `crates/zero-abi`. Evidence path in the matrix is `schema.rs` only.

### Falsifiability Criteria

`cargo test -p zero-abi` runs a `proptest!` encode/decode identity for a wire type.

### One-Line Invocation

```bash
rg -n "proptest!" crates/zero-abi ; ls crates/zero-abi/tests 2>&1
```

### Results Inline

```
result_status: CLOSED
result_summary: crates/zero-abi/tests/abi_proptest.rs encode/decode identity on Handshake and Shutdown frames.
result_evidence_paths:
  - crates/zero-abi/tests/abi_proptest.rs
  - conformance/contracts/supported_surface_matrix.toml
result_impact: F-ABI-PROPTEST-ROUNDTRIP present
spawned_remediation_bead: F-ABI-PROPTEST-ROUNDTRIP
spawned_experiments: [IDEA-0004]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed when crates/zero-abi has a proptest! encode/decode identity for a wire type and cargo test -p zero-abi runs it.

---

## SURF-0008 -- F-REF-ENGINE-ADOPTION-LOCKSTEP (out-of-repo)

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0008` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-REF-ENGINE-ADOPTION-LOCKSTEP` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> The hub can enforce sibling engines to use ClampEnd rather than Reject.

### Motivation

TokenZero sibling gauntlet found EmbeddedStore reject vs RecoveryStore clamp TrueDivergence. Hub defines `CANONICAL_LINE_END_POLICY = ClampEnd` but cannot compile-fail engines it does not own.

### Minimal Reproducer

```bash
rg -n "CANONICAL_LINE_END_POLICY|Reject" crates/zero-ref/src/lib.rs
```

### Expected Signal

Hub source cannot fail a sibling engine CI job. Any "lockstep" claim that is comment-only is an overclaim.

### Falsifiability Criteria

A live zsx receipt or an engine CI job fails when an engine uses Reject. That evidence cannot be produced from this repo alone.

### One-Line Invocation

```bash
rg -n "Retry only when a hub gate fails a sibling engine" conformance/contracts/supported_surface_matrix.toml
```

### Results Inline

```
result_status: CLOSED
result_summary: Out-of-repo. Pass 11 must not pretend a hub comment is enforcement. Related: clamp-end-vs-reject DEFERRED.
result_evidence_paths:
  - conformance/contracts/supported_surface_matrix.toml
  - docs/progress/conformance-negative-results.md
result_impact: zero-ref family partial
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry only when a hub gate fails a sibling engine that uses Reject instead of ClampEnd, evidenced by a live zsx receipt or an engine CI job -- not a comment-only lockstep claim.

---

## SURF-0009 -- F-STORE-ENSURE-LAYOUT partial

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0009` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-STORE-ENSURE-LAYOUT` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `ensure_layout` creates `unified_root` (when present) and `engine_dir`, and does not create `blobs/` or `gc/`.

### Motivation

Matrix partial. Source `crates/zero-store/src/store_root.rs` lines 416--435: `create_dir_all(root)` then `create_dir_all(engine_dir)` only. `blobs_dir()` is a path helper, not a mkdir.

### Minimal Reproducer

```rust
// after ensure_layout(resolved) the following must fail today
assert!(resolved.blobs_dir().is_dir()); // false unless a CAS put already ran
```

### Expected Signal

A fresh `ensure_layout` leaves `cas_host()/blobs` and `cas_host()/gc` absent.

### Falsifiability Criteria

After a successful `ensure_layout`, `blobs/` and `gc/` exist, **or** the function returns a loud typed error if a caller expected them. A unit test fails if the chosen contract is violated.

### One-Line Invocation

```bash
rg -n -A20 "pub fn ensure_layout" crates/zero-store/src/store_root.rs
```

### Results Inline

```
result_status: CLOSED
result_summary: ensure_layout now creates engine_dir, blobs/, and gc/ under cas_host(). Tested in crates/zero-store/tests/ensure_layout.rs.
result_evidence_paths:
  - crates/zero-store/src/store_root.rs
  - crates/zero-store/tests/ensure_layout.rs
  - conformance/contracts/supported_surface_matrix.toml
result_impact: F-STORE-ENSURE-LAYOUT present
spawned_remediation_bead: F-STORE-ENSURE-LAYOUT
spawned_experiments: [IDEA-0005]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed when ensure_layout creates blobs/ and gc/ and a unit test fails if those directories are absent after a successful call.

---

## SURF-0010 -- F-CODEMODE-CANCEL partial

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0010` |
| `pillar` | `surface` |
| `status` | `CONFIRMED_GAP` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-CODEMODE-CANCEL` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> CancellationSignal + MCP late-Ok salvage exist, but no hub test outside rival-dirty `crates/zsx-core/src/fszero.rs` proves `commit_race` + `retryable=false` + payload retained.

### Motivation

Matrix partial. `mcp-late-ok-salvage` DEFERRED. Rival-dirty tree must not be edited this pass.

### Minimal Reproducer

```bash
rg -n "commit_race" crates/zero-mcp/src/mcp_transport.rs crates/zero-codemode/src/cancellation.rs
rg -n "commit_race" tests/unit --glob '*cancel*' || true
```

### Expected Signal

Production code exists; the covering test lives in rival-dirty `fszero_tests.rs` or is absent from hub-owned tests.

### Falsifiability Criteria

A hub test that does not live only in rival-dirty `fszero.rs` fails if a cancelled/timed-out `zero_execute` that later receives Ok reports anything other than `commit_race` / `retryable=false` / payload kept.

### One-Line Invocation

```bash
rg -n "commit_race" crates/zero-mcp/src/mcp_transport.rs tests/unit --glob '!**/fszero*'
```

### Results Inline

```
result_status: CONFIRMED_GAP
result_summary: Code present; matrix retry names the missing hub test. Do not touch rival-dirty files to "close" this.
result_evidence_paths:
  - crates/zero-codemode/src/cancellation.rs
  - crates/zero-mcp/src/mcp_transport.rs
  - docs/progress/conformance-negative-results.md
result_impact: zero-codemode family 0.850000
spawned_remediation_bead: F-CODEMODE-CANCEL
spawned_experiments: [IDEA-0015]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry only when a cancelled or timed-out zero_execute that later receives Ok reports commit_race with retryable=false and keeps the committed payload, covered by a hub test that does not live only in rival dirty crates/zsx-core/src/fszero.rs.

---

## SURF-0011 -- F-ZSX-Q99-REPORT partial

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0011` |
| `pillar` | `surface` |
| `status` | `CONFIRMED_GAP` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-ZSX-Q99-REPORT` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Session Q99 types exist, but FSZero/GraphZero adapters return no `WorkerTokenAccountingV1`, so L1/L2 windows stay empty in production.

### Motivation

Matrix partial. `residency.rs` documents the residual in-file. Filling L1/L2 is engine work (out-of-repo). Hub-side close is a loud-unavailable test (IDEA-0012).

### Minimal Reproducer

```bash
rg -n "no worker token accounting|no_demand_observations" crates/zsx-core/src/residency.rs
```

### Expected Signal

File states L1/L2 stay empty until engine-side accounting exists.

### Falsifiability Criteria

Adapters supply measured `WorkerTokenAccountingV1` so L1/L2 Q99 windows are non-empty, **or** the report loudly refuses to emit a numeric Q99 when observations are empty (already partially documented).

### One-Line Invocation

```bash
rg -n "Residual|WorkerTokenAccountingV1|empty" crates/zsx-core/src/residency.rs | head
```

### Results Inline

```
result_status: CONFIRMED_GAP
result_summary: Documented residual. Pass 11 must not fake engine accounting. Hub test of unavailable-on-empty is in-hub (IDEA-0012).
result_evidence_paths:
  - crates/zsx-core/src/residency.rs
  - conformance/contracts/supported_surface_matrix.toml
result_impact: zsx-core family 0.961538
spawned_remediation_bead: F-ZSX-Q99-REPORT
spawned_experiments: [IDEA-0012]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry only when FSZero/GraphZero adapters supply measured WorkerTokenAccountingV1 so L1/L2 Q99 windows are non-empty in production, or the report loudly refuses to emit a numeric Q99 when observations are empty.

---

## SURF-0012 -- F-REF-ERROR-TAXONOMY partial

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0012` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-REF-ERROR-TAXONOMY` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Production paths in `crates/zero-ref/src/lib.rs` do not construct every `ZeroRefErrorClass` variant; `ALL` is public but not shown reachable.

### Motivation

Matrix partial. Grep of `lib.rs` constructs Malformed, Unsupported, DigestMismatch, RangeOutOfBounds, NotUtf8. Missing/Io/PolicyDenied/IncompatibleVersion/LegacyAmbiguity are not constructed there (`Missing` appears only as `.as_str()` in the harness).

### Minimal Reproducer

```bash
rg -n "ZeroRefErrorClass::" crates/zero-ref/src/lib.rs
```

### Expected Signal

Five of ten variants never appear as constructors in `lib.rs`.

### Falsifiability Criteria

Every `ALL` variant is constructed on a production path, **or** a test asserts reachability (including explicit "parser never emits this class" cases).

### One-Line Invocation

```bash
rg -n "ZeroRefErrorClass::" crates/zero-ref/src/lib.rs
```

### Results Inline

```
result_status: CLOSED
result_summary: negotiate() constructs IncompatibleVersion. Missing/Io/PolicyDenied/LegacyAmbiguity documented as reserved for store/resolution. ALL constructible; parser never emits reserved classes. Feature stays partial honestly.
result_evidence_paths:
  - crates/zero-ref/src/lib.rs
  - crates/zero-ref/tests/zeroref_api.rs
result_impact: F-REF-ERROR-TAXONOMY remains partial
spawned_remediation_bead: F-REF-ERROR-TAXONOMY
spawned_experiments: [IDEA-0013]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed with honest partial: tests assert ALL is reachable and reserved classes are explicit.

---

## SURF-0013 -- F-STORE-QUARANTINE-REAP partial

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0013` |
| `pillar` | `surface` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `F-STORE-QUARANTINE-REAP` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `CAS_QUARANTINE_DIR` / `CAS_TEMP_REAP_AGE` are constants only; put/gc do not move corrupt objects or reap temps, and no unit test covers both.

### Motivation

Matrix partial. Source **does** call `reap_stale_temps` from put and defines `quarantine_object`. The remaining hole may be tests, not functions. Do not mark `CONFIRMED_GAP` for "functions missing" -- that would be false.

### Minimal Reproducer

```bash
rg -n "quarantine_object|reap_stale_temps|CAS_TEMP_REAP_AGE" crates/zero-store/src/cas.rs tests/unit/zero-store
```

### Expected Signal

If the hypothesis is true: no unit test fails when quarantine/reap are stubbed. If false: tests already cover both constants and the row can move to `present`.

### Falsifiability Criteria

A unit test covering both constants exists and fails if put/gc stop moving corrupt objects or reaping temps older than `CAS_TEMP_REAP_AGE`.

### One-Line Invocation

```bash
rg -n "quarantine_object|reap_stale_temps" crates/zero-store tests/unit/zero-store
```

### Results Inline

```
result_status: OPEN
result_summary: Functions exist (cas.rs:379,581,804). Experiment is "are they tested?". Not CONFIRMED_GAP for absence of the path.
result_evidence_paths:
  - crates/zero-store/src/cas.rs
result_impact: unknown until the test inventory is run
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry only when CAS put/gc actually moves corrupt objects into CAS_QUARANTINE_DIR and reaps temps older than CAS_TEMP_REAP_AGE, with a unit test covering both constants.

---

## SURF-0014 -- engine-only surfaces excluded

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0014` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Absorbing FSZero / GraphZero / TokenZero private surfaces into the hub FeatureUniverse would make strict coverage 1.0 without those engines being workspace members.

### Motivation

Three excluded rows with non-zero weight. Excluded-as-debt is the point. Surface-deferrals DEFERRED all three.

### Minimal Reproducer

```bash
python3 scripts/check_feature_coverage_dashboard.py
```

### Expected Signal

strict (0.899590) < effective (0.910788) because excluded still counts.

### Falsifiability Criteria

An engine becomes a public Cargo workspace member with its own CONTRACT.md.

### One-Line Invocation

```bash
python3 scripts/check_feature_coverage_dashboard.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Exclusion is correct. Pass 11 must not fold engine surfaces into the hub to paint the dashboard green.
result_evidence_paths:
  - docs/progress/surface-deferrals.md
  - conformance/contracts/supported_surface_matrix.toml
result_impact: 3 x 0.004098 weight units of strict debt
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Worth reconsidering when FSZero/GraphZero/TokenZero is a public Cargo workspace member of this hub with its own CONTRACT.md.

---

## SURF-0015 -- dashboard gate red is honest

| Field | Value |
|---|---|
| `experiment_id` | `SURF-0015` |
| `pillar` | `surface` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CLOSED-0007` |

### Hypothesis

> Rounding partial/missing/excluded families up would make the release gate green at strict 0.899590.

### Motivation

Phase 9 headline. Partial never rounds up. Excluded is strict-100% debt. Painting green would be a lie.

### Minimal Reproducer

```bash
python3 scripts/check_feature_coverage_dashboard.py
```

### Expected Signal

`gate=red`, `strict_100_certifiable=false`, strict=0.899590.

### Falsifiability Criteria

Gate is green only when missing=0 and excluded debt is gone (or a documented waiver, which this card refuses).

### One-Line Invocation

```bash
python3 scripts/check_feature_coverage_dashboard.py
```

### Results Inline

```
result_status: CLOSED
result_summary: Red is the correct answer. Do not flip the gate. Close SURF-0001..0004 first.
result_evidence_paths:
  - docs/progress/phase9_baseline.md
  - docs/progress/baseline_round_0.json
result_impact: release uses 0.899590 lower/strict bound
spawned_remediation_bead: N/A
spawned_experiments: [IDEA-0017]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Worth reconsidering when scripts/check_feature_coverage_dashboard.py reports gate=green and strict_100_certifiable=true.

---

## CONF-0001 -- SPEC-COMP-002 unverified

| Field | Value |
|---|---|
| `experiment_id` | `CONF-0001` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `SPEC-COMP-002` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> A multi-mutation FSZero execute whose later step fails rolls back the earlier mutation in the same execute.

### Motivation

`[SPEC-COMP-002]` UNVERIFIED. Journal+undo types exist; live subsequent-failed-step rollback is not driven. Not CONFIRMED_GAP: we do not have a failing execute, only a missing verifier.

### Minimal Reproducer

A hub verifier that runs two mutations in one execute, fails the second, and asserts the first is rolled back. Must not live only in rival-dirty `fszero.rs`.

### Expected Signal

If true: verifier green; tag moves off UNVERIFIED. If false: TrueDivergence + FailureBundle.

### Falsifiability Criteria

Verifier cannot be written without engine-private APIs, or the live execute does not roll back -- then spawn a CONFIRMED_GAP child.

### One-Line Invocation

```bash
rg -n "SPEC-COMP-002|verify_spec_comp_002" crates/zerostack-harness/src/spec_oracle.rs docs/spec/SPEC-TAGS.md
```

### Results Inline

```
result_status: OPEN
result_summary: Tag still UNVERIFIED. No live multi-mutation execute in the hub harness.
result_evidence_paths:
  - docs/spec/SPEC-TAGS.md
  - docs/progress/conformance-negative-results.md
result_impact: unknown
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Blocked until a hub verifier drives a multi-mutation execute whose subsequent step fails and proves earlier mutation rolled back; track as SPEC-COMP-002.

---

## CONF-0002 -- SPEC-HON-002 unverified

| Field | Value |
|---|---|
| `experiment_id` | `CONF-0002` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `SPEC-HON-002` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> A hub production path emits `result_finalization_receipt.v1` with null `visibleTokenCount` and `requires_tokenzero_certification` on an uncertified spill.

### Motivation

UNVERIFIED: only bench fixtures emit that receipt. Not CONFIRMED_GAP that production is wrong -- we have not driven an oversize `zsx exec` and inspected the envelope.

### Minimal Reproducer

```bash
# after a large-result zsx exec, inspect the envelope
rg -n "visibleTokenCount|result_finalization_receipt" crates benchmarks
```

### Expected Signal

If true: a production spill sets both fields. If production never emits the receipt, refine to a CONFIRMED_GAP on emission (not on field values).

### Falsifiability Criteria

A production path emits the receipt with those fields, or a verifier proves no production path can spill without it.

### One-Line Invocation

```bash
rg -n "result_finalization_receipt|visibleTokenCountStatus" crates benchmarks
```

### Results Inline

```
result_status: OPEN
result_summary: Fixture-only emission is documented. Need a production-path probe before CONFIRMED_GAP.
result_evidence_paths:
  - docs/spec/SPEC-TAGS.md
result_impact: unknown
spawned_remediation_bead: N/A
spawned_experiments: [IDEA-0024]
closed_at_utc:
```

### Closure Predicate

Blocked until a hub production path emits result_finalization_receipt.v1 with null visibleTokenCount and status requires_tokenzero_certification on an uncertified spill; track as SPEC-HON-002.

---

## CONF-0003 -- SPEC-HON-006 unverified (permission)

| Field | Value |
|---|---|
| `experiment_id` | `CONF-0003` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `SPEC-HON-006` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> The hub accepts a TokenZero Exact certification receipt without rejecting it (permission, not a hub requirement to emit Exact).

### Motivation

UNVERIFIED because Exact is TokenZero-owned. Implementing hub-side Exact would violate `[SPEC-HON-001]`. Pass 11 must not invent billed_tokens.

### Minimal Reproducer

Feed a fixture TokenZero Exact receipt through the hub envelope and assert it is not rejected.

### Expected Signal

Hub does not reject certified Exact. Hub still refuses estimates labeled Exact (`[SPEC-HON-004]`).

### Falsifiability Criteria

Hub rejects a well-formed TokenZero Exact receipt, or someone "closes" the tag by emitting Exact from the hub.

### One-Line Invocation

```bash
rg -n "SPEC-HON-006|Exact" docs/spec/SPEC-TAGS.md conformance/CONTRACT.md | head
```

### Results Inline

```
result_status: OPEN
result_summary: Permission tag. Out-of-repo to emit Exact. In-hub experiment is accept-without-reject on a fixture receipt.
result_evidence_paths:
  - docs/spec/SPEC-TAGS.md
  - docs/progress/conformance-negative-results.md
result_impact: unknown
spawned_remediation_bead: N/A
spawned_experiments: [IDEA-0025]
closed_at_utc:
```

### Closure Predicate

Blocked until a TokenZero certification receipt is accepted by the hub without rejection and a verifier asserts that permission; track as SPEC-HON-006.

---

## CONF-0004 -- SPEC-HUB-002 unverified

| Field | Value |
|---|---|
| `experiment_id` | `CONF-0004` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `SPEC-HUB-002` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> A fail-loud suite on live `zsx` receipts catches silent-success and heuristic-labeled-exact. A single static field-name check is not enough.

### Motivation

UNVERIFIED: global law; no single static surface is a complete proof.

### Minimal Reproducer

A suite that feeds a silent-success envelope and a heuristic-Exact envelope and expects hard failure.

### Expected Signal

Both cases fail loud on live receipts, not comments.

### Falsifiability Criteria

Suite cannot be written without inventing product CLI (forbidden), or live receipts already fail loud -- then close as NO_EVIDENCE with the suite as the verifier.

### One-Line Invocation

```bash
rg -n "SPEC-HUB-002|fail loud|heuristic" docs/spec/SPEC-TAGS.md crates/zerostack-harness/src/spec_oracle.rs
```

### Results Inline

```
result_status: OPEN
result_summary: Still UNVERIFIED. Do not close from a field-name grep.
result_evidence_paths:
  - docs/spec/SPEC-TAGS.md
result_impact: unknown
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Blocked until a fail-loud suite covers silent-success and heuristic-labeled-exact on live zsx receipts (not comments); track as SPEC-HUB-002.

---

## CONF-0005 -- SPEC-HUB-005 unverified

| Field | Value |
|---|---|
| `experiment_id` | `CONF-0005` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `SPEC-HUB-005` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Changing a C-23 / C-24 / C-26 Wire/version pin without bumping the ABI digest is possible today (no mutation test).

### Motivation

UNVERIFIED: digest bump is a mutation test, not a static field-name check. C-25 semantic-mutation bumps stay Ambiguous.

### Minimal Reproducer

A test that mutates a pin constant and fails if the digest is unchanged.

### Expected Signal

If the hypothesis is true: mutation test is absent (current state). If a mutation test already exists, refute.

### Falsifiability Criteria

A mutation test changes a C-23/24/26 pin and fails if the ABI digest does not bump.

### One-Line Invocation

```bash
rg -n "SPEC-HUB-005|C-23|schema digest" crates/zero-abi crates/zerostack-harness docs/spec/SPEC-TAGS.md | head
```

### Results Inline

```
result_status: OPEN
result_summary: No mutation test cited in SPEC-TAGS. Not CONFIRMED_GAP that digest fails to bump -- we have not mutated a pin.
result_evidence_paths:
  - docs/spec/SPEC-TAGS.md
result_impact: unknown
spawned_remediation_bead: N/A
spawned_experiments: [IDEA-0009]
closed_at_utc:
```

### Closure Predicate

Blocked until a mutation test changes a C-23/24/26 pin and fails if the ABI digest does not bump; track as SPEC-HUB-005.

---

## CONF-0006 -- preflight AGENTS.md hash drift

| Field | Value |
|---|---|
| `experiment_id` | `CONF-0006` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Gitignored `AGENTS.md` sha256 drifted from the Phase 3/4 pin after the Phase 8 mandate append, so oracle-preflight-doctor is red and not certifying.

### Motivation

Phase 9 first_failure_diagnosis. Expected `da53da8383380c712f781c5f5cc85f48e56bc43281f824af11e47fabb7cca6f4`, got `7211bb0e78819cf5b6405219a209329c5c12616ab8eff5f09cd1128271b4c5b2`. Smoke test does not require aggregate green.

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json
```

### Expected Signal

`aggregate_outcome=red`, `certifying=false`, check `spec_source_sha256` red with those two hashes.

### Falsifiability Criteria

Working-copy hash equals the contract pin and preflight is green. Re-pinning requires operator bless of the contract and its Tier1Raw golden (`check_golden_integrity.py` never blesses).

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json
```

### Results Inline

```
result_status: CLOSED
result_summary: agents-law is certifying=false. Preflight skips it for spec_source_sha256 and reports spec_source_sha256_advisory as yellow on drift/absent. More honest than blessing a moving gitignored hash.
result_evidence_paths:
  - conformance/contracts/spec_version_contract.toml
  - crates/zerostack-harness/src/spec_oracle.rs
  - crates/zerostack-harness/src/oracle_preflight_doctor.rs
result_impact: certifying no longer blocked by gitignored AGENTS.md
spawned_remediation_bead: N/A
spawned_experiments: [IDEA-0002, IDEA-0010]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed by dropping AGENTS.md from the certifying pin set and keeping a non-certifying advisory check.

---

## PERF-0001 -- savings-bench cv_pct null

| Field | Value |
|---|---|
| `experiment_id` | `PERF-0001` |
| `pillar` | `perf` |
| `status` | `CONFIRMED_GAP` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CLOSED-0006` |

### Hypothesis

> The committed savings-bench seed (`primary_score = 0.044`) has `cv_pct = null` and `keep_eligible = false`; treating it as a win would violate the keep-gate.

### Motivation

Phase 5/9. `cv-pct-null-savings-baseline` REJECTED as a keep. The seed state is proven.

### Minimal Reproducer

```bash
python3 -c "import json; d=json.load(open('.bench-history/savings-bench.latest.json')); print(d['cv_pct'], d['keep_eligible'])"
```

### Expected Signal

Prints `None False` (JSON null / false).

### Falsifiability Criteria

A replicated `release-perf` savings-bench run records `cv_pct <= 5` and `keep_eligible true`. Until then the seed is not a win.

### One-Line Invocation

```bash
python3 scripts/check_bench_history.py
```

### Results Inline

```
result_status: CONFIRMED_GAP
result_summary: Seed is honest about being ineligible. Gap is "no keep-eligible cv_pct", not a lying number. Spawn OPEN-0015 for the replicated run.
result_evidence_paths:
  - .bench-history/savings-bench.latest.json
  - docs/progress/baseline_round_0.json
  - docs/progress/perf-negative-results.md
result_impact: keep_eligible=false; primary_score=0.044 unchanged
spawned_remediation_bead: N/A
spawned_experiments: [OPEN-0015]
closed_at_utc: 2026-08-17
```

### Closure Predicate

Retry only if this workload class exhibits measurable cv_pct below 5.0 on a replicated `release-perf` savings-bench run with keep_eligible true.

---

## PERF-0002 -- fat-LTO waiver (decided)

| Field | Value |
|---|---|
| `experiment_id` | `PERF-0002` |
| `pillar` | `perf` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> Flipping `release-perf.lto` from `"fat"` to `"thin"` is required for honest flamegraphs.

### Motivation

Pass 5 aligned line tables + symbols. Fat LTO is shared with `release-node`. Named waiver in `Cargo.toml`. `fat-lto-to-thin-switch` DEFERRED.

### Minimal Reproducer

```bash
rg -n 'lto = "fat"' Cargo.toml
```

### Expected Signal

Profile is fat, not thin. Line tables stay (`debug = "line-tables-only"`, `strip = false`).

### Falsifiability Criteria

A dedicated rch release-perf flamegraph shows missing frames under fat LTO, or `release-node` leaves fat LTO.

### One-Line Invocation

```bash
rg -n -A12 '\[profile.release-perf\]' Cargo.toml
```

### Results Inline

```
result_status: CLOSED
result_summary: Decided waiver. Do not flip in pass 11. Not a keep of a product optimization.
result_evidence_paths:
  - Cargo.toml
  - docs/progress/perf-negative-results.md
result_impact: none
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Worth reconsidering when the release-node profile family also leaves fat LTO, or when a dedicated rch release-perf flamegraph run shows missing frames under fat LTO.

---

## PERF-0003 -- cass unavailable

| Field | Value |
|---|---|
| `experiment_id` | `PERF-0003` |
| `pillar` | `perf` |
| `status` | `CONFIRMED_GAP` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `cass-on-path` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> `cass` is not on PATH, so the 60-day cross-machine session mine cannot run; fallback git-log is not a full mine.

### Motivation

Phase 8 blocker. Mandate: do not silently skip. `command -v cass` failed.

### Minimal Reproducer

```bash
command -v cass ; echo exit:$?
```

### Expected Signal

Command not found. `jsm list` shows cass as installable, not installed.

### Falsifiability Criteria

`command -v cass` succeeds and `cass health --robot` is green.

### One-Line Invocation

```bash
command -v cass || echo 'cass missing'
```

### Results Inline

```
result_status: CONFIRMED_GAP
result_summary: Still missing as of Phase 8/9. Not a skip. Fallback git log is recorded in perf-negative-results.md.
result_evidence_paths:
  - docs/progress/perf-negative-results.md
  - ZeroStack__gauntlet_workspace/cass_blocker.md
result_impact: blocker for any keep-eligible perf claim
spawned_remediation_bead: cass-on-path
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Blocked until cass lands on PATH (`command -v cass` succeeds and `cass health --robot` is green); track as cass-on-path.

---

## OPEN-0014 -- does DSR already run cargo test?

| Field | Value |
|---|---|
| `experiment_id` | `OPEN-0014` |
| `pillar` | `surface` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `hypothesis-spawner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0006` |

### Hypothesis

> DSR already runs targeted `cargo test` on the repo-level gate, satisfying the `F-CI-PR-GATES` retry without adding a GitHub Actions test job.

### Motivation

AGENTS.md: DSR is the CI source of truth; GH is throttled. Adding a GH test job may be the wrong close.

### Minimal Reproducer

```bash
command -v dsr && dsr doctor ; rg -n "cargo test" "$(command -v dsr)" 2>/dev/null | head
```

### Expected Signal

If true: a DSR config or log shows cargo test on the default gate. If false: DSR is fmt/clippy/build only, and the CI gap remains.

### Falsifiability Criteria

`dsr` missing, or DSR gate has no cargo test.

### One-Line Invocation

```bash
command -v dsr && dsr doctor || echo 'dsr missing'
```

### Results Inline

```
result_status: OPEN
result_summary: Not run this pass. Do not add a GH test job until this is resolved.
result_evidence_paths: []
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Worth reconsidering when DSR logs show cargo test on the default repo gate, or when dsr doctor reports no such job.

---

## OPEN-0015 -- replicated savings-bench cv_pct

| Field | Value |
|---|---|
| `experiment_id` | `OPEN-0015` |
| `pillar` | `perf` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `hypothesis-spawner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `PERF-0001` |

### Hypothesis

> Ten rch `release-perf` repeats of the savings-bench headline (`token.read compact_50k.txt`, max_visible_tokens=200) produce a measurable `cv_pct` at or below 5.0.

### Motivation

Seed `cv_pct` is null. This is the measurement, not a product patch. Host Mac is the wrong place (comprehensive-bench-93-on-mac DEFERRED).

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack RUSTFLAGS='-C force-frame-pointers=yes' \
  python3 scripts/bench/store_cas.py --repeat 10
```

(Replace with the actual savings-bench runner once invoked; do not invent numbers.)

### Expected Signal

A new `.bench-history` candidate with numeric `cv_pct`. If `cv_pct > 5`, still not keep-eligible.

### Falsifiability Criteria

Repeats cannot be produced (TokenZero missing locally), or `cv_pct > 5`, or the headline is not reproducible on rch.

### One-Line Invocation

```bash
python3 scripts/check_bench_history.py
```

### Results Inline

```
result_status: OPEN
result_summary: Not run. Phase 9 only re-checked the seed against itself.
result_evidence_paths:
  - .bench-history/savings-bench.latest.json
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry only if this workload class exhibits measurable cv_pct below 5.0 on a replicated `release-perf` savings-bench run with keep_eligible true.

---

## OPEN-0016 -- comprehensive-bench 93 on rch

| Field | Value |
|---|---|
| `experiment_id` | `OPEN-0016` |
| `pillar` | `perf` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `advanced-methods-miner` |
| `bead_id` | `F-STORE-BENCH-HISTORY` |
| `parent_hypothesis_id` | `N/A` |

### Hypothesis

> An rch-offloaded comprehensive-bench JSON v3 with `cv_pct <= 5` can replace the 12-row savings seed as the pass-over-pass baseline.

### Motivation

`comprehensive-bench-93-on-mac` DEFERRED. Phase 9 did not run 93 scenarios. Do not invent the matrix on this Mac.

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo run -p zerostack-harness --bin comprehensive_bench -- --full
```

(Binary may not exist -- that itself is a signal; then the experiment refines.)

### Expected Signal

JSON v3 with schema, `detected_environment`, `summary`, `ci_regression_gate`, `sections[]`, numeric `cv_pct`.

### Falsifiability Criteria

No comprehensive-bench binary, or rch run cannot produce `cv_pct <= 5`.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo run -p zerostack-harness --bin comprehensive_bench -- --help
```

### Results Inline

```
result_status: OPEN
result_summary: Not run. Phase 5/9 explicitly skipped the 93-scenario matrix.
result_evidence_paths:
  - docs/progress/perf-negative-results.md
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Blocked until an rch-offloaded comprehensive-bench JSON v3 with cv_pct at or below 5 and a committed `.bench-history` refresh lands; track as F-STORE-BENCH-HISTORY.

---

## PERF-0007 -- profile-first CAS put

| Field | Value |
|---|---|
| `experiment_id` | `PERF-0007` |
| `pillar` | `perf` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `advanced-methods-miner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CLOSED-0011` |

### Hypothesis

> `SharedCas::put` is a frame ≥0.1% self-time under `release-perf` store_cas, so a later product patch could be keep-eligible.

### Motivation

Queued in perf-negative-results Open Candidates. Must satisfy test-profile and cv_pct predicates first. Profile-first: no source touch.

### Minimal Reproducer

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack RUSTFLAGS='-C force-frame-pointers=yes' \
  samply record --output /tmp/cas-put.json -- cargo test -p zerostack-harness --test store_cas_microbench --release -- --test-threads=1
```

(Use `--profile release-perf` once a bench binary exists. Test profile is not keep-eligible.)

### Expected Signal

samply/flamegraph names `SharedCas::put` or `atomic_write_file` at ≥0.1% self-time.

### Falsifiability Criteria

Frame <0.1% (micro-lever trap) or cv_pct >5.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --test store_cas_microbench -- --test-threads=1
```

### Results Inline

```
result_status: OPEN
result_summary: Not profiled. Phase 9 ran the test only.
result_evidence_paths: []
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry only if a profiler attributes a clearly-above-noise share to SharedCas::put or atomic_write_file on a release-perf store_cas workload with cv_pct at or below 5.

---

## Idea-wizard promotions (OPEN unless they duplicate a CONFIRMED_GAP)

Raw 30 / top-5 / +10 live in the workspace: `ZeroStack__gauntlet_workspace/round_0/ideas/idea_wizard_phase2.md`. Cards below are the non-duplicate deeper picks.

### IDEA-0002 -- pin tracked mandate instead of gitignored AGENTS.md

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0002` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CONF-0006` |

### Hypothesis

> Pointing the agents-law spec source at tracked `docs/progress/AGENTS-MANDATE.md` (or hashing a tracked excerpt) removes preflight flake without committing gitignored `AGENTS.md`.

### Motivation

`agents-md-as-feature-evidence` REJECTED using AGENTS.md as FeatureUniverse evidence. This idea is a **spec-source pin**, not a feature row. Non-obvious: the tracked mandate already exists.

### Minimal Reproducer

Compute sha256 of `docs/progress/AGENTS-MANDATE.md` and compare to a proposed contract pin.

### Expected Signal

Preflight can go green without depending on a gitignored file.

### Falsifiability Criteria

Mandate file is not a complete law subset; hash would not represent AGENTS.md. Then re-pin the live file instead (CONF-0006).

### One-Line Invocation

```bash
shasum -a 256 docs/progress/AGENTS-MANDATE.md AGENTS.md
```

### Results Inline

```
result_status: OPEN
result_summary: Not run. Alternative to a live-file re-pin.
result_evidence_paths:
  - docs/progress/AGENTS-MANDATE.md
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Blocked until spec_version_contract.toml agents-law pin is either the working-copy AGENTS.md hash or a tracked-file hash that preflight accepts; track as agents-law-hash-pin.

---

### IDEA-0010 -- preflight smoke must require certifying

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0010` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CONF-0006` |

### Hypothesis

> `oracle_smoke::preflight_does_not_panic_and_is_green` allows aggregate red, so CI can stay green while certifying=false.

### Motivation

Phase 9 noted this explicitly. Gate-honesty hole: the test name says green, the body does not require it.

### Minimal Reproducer

```bash
rg -n "preflight_does_not_panic_and_is_green|certifying" crates/zerostack-harness/tests/oracle_smoke.rs
```

### Expected Signal

If true: test passes on Phase 9 red preflight (already observed).

### Falsifiability Criteria

Test already requires `aggregate_outcome == green`. Then the name is accurate and this idea is NO_EVIDENCE.

### One-Line Invocation

```bash
rg -n "fn preflight" crates/zerostack-harness/tests/oracle_smoke.rs
```

### Results Inline

```
result_status: OPEN
result_summary: Phase 9 narrative supports the claim; not yet re-read as a failing assertion. Do not tighten the test until CONF-0006 is re-pinned, or the suite goes red for the known hash drift.
result_evidence_paths:
  - docs/progress/phase9_baseline.md
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Worth reconsidering when oracle-preflight-doctor is green; tightening the smoke test before the pin is blessed would fail the suite on a known, unfixed hash drift.

---

### IDEA-0012 -- Q99 unavailable-on-empty hub test

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0012` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0011` |

### Hypothesis

> With empty worker-token observations, the session Q99 report emits `unavailable` and never a numeric Q99. A missing test would let a vacuous 0% slip through.

### Motivation

`residency.rs` claims this. In-hub close for SURF-0011 without engine accounting.

### Minimal Reproducer

Construct a `SessionResidencyGate` with no observations and assert report status.

### Expected Signal

Status `unavailable`; no bare percentage.

### Falsifiability Criteria

Report already has such a test, or it emits a number on empty windows (then CONFIRMED_GAP).

### One-Line Invocation

```bash
rg -n "unavailable|no_demand_observations" crates/zsx-core/src/residency.rs tests/unit/zsx-core
```

### Results Inline

```
result_status: OPEN
result_summary: Claim is documented. Test inventory not completed this pass.
result_evidence_paths:
  - crates/zsx-core/src/residency.rs
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry only when a unit test fails if an empty Q99 window emits a numeric percentage.

---

### IDEA-0013 -- error-class reachability table

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0013` |
| `pillar` | `conformance` |
| `status` | `CLOSED` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0012` |

### Hypothesis

> A table-driven test over `ZeroRefErrorClass::ALL` with one input (or an explicit `parser_never_emits`) per class is the smallest honest close.

### Motivation

SURF-0012 proves 5 classes unused. This is the remediation shape, not a new gap.

### Minimal Reproducer

Add `tests/unit/zero-ref/error_class_reachability.rs` (pass 11).

### Expected Signal

Test fails listing the five unconstructed classes until either constructors or `parser_never_emits` annotations exist.

### Falsifiability Criteria

Such a test already exists.

### One-Line Invocation

```bash
rg -n "ZeroRefErrorClass::ALL" tests/unit crates/zero-ref
```

### Results Inline

```
result_status: CLOSED
result_summary: zeroref_api.rs constructs ALL, asserts parser never emits reserved classes, and negotiate emits IncompatibleVersion.
result_evidence_paths:
  - crates/zero-ref/tests/zeroref_api.rs
result_impact: SURF-0012 closed as honest partial
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc: 2026-08-17
```

### Closure Predicate

Closed when a reachability test for ZeroRefErrorClass::ALL landed.

---

### IDEA-0015 -- cancel test without touching fszero.rs

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0015` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0010` |

### Hypothesis

> `CancellationSignal` + `commit_race_error` can be unit-tested at the MCP transport layer with a fake late Ok, without opening rival-dirty `fszero.rs`.

### Motivation

Non-obvious: the settlement law is in `zero-mcp`, not in the FSZero adapter.

### Minimal Reproducer

Drive `mcp_transport` with a cancelled inflight + late Ok fixture.

### Expected Signal

Kind `commit_race`, `retryable=false`, payload attached.

### Falsifiability Criteria

Transport API is not reachable without a full session, forcing the test into zsx-core (still not fszero.rs).

### One-Line Invocation

```bash
rg -n "fn commit_race_error|late_ok" crates/zero-mcp/src/mcp_transport.rs
```

### Results Inline

```
result_status: OPEN
result_summary: Design only. Pass 11 may implement if SURF-0010 is picked.
result_evidence_paths:
  - crates/zero-mcp/src/mcp_transport.rs
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry only if this workload class exhibits measurable commit_race coverage below 1.0 on a hub test outside rival-dirty fszero.rs.

---

### IDEA-0017 -- conformal ratchet on strict 0.899590

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0017` |
| `pillar` | `surface` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `advanced-methods-miner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0015` |

### Hypothesis

> Persisting `strict_coverage = 0.899590` as a ratchet floor (`apply-ratchet.sh`) detects a silent matrix weight edit that lowers the bound.

### Motivation

Frontier-math: the conformal lower bound is already the release number. It is not yet a committed ratchet state.

### Minimal Reproducer

```bash
python3 scripts/check_feature_coverage_dashboard.py
# then compare to a committed reports/ratchet_state.json if present
```

### Expected Signal

If ratchet missing: OPEN stays. If present and monotone: close as NO_EVIDENCE (already wired).

### Falsifiability Criteria

`reports/ratchet_state.json` already gates 0.899590.

### One-Line Invocation

```bash
ls reports/ratchet_state.json 2>&1 ; python3 scripts/check_feature_coverage_dashboard.py
```

### Results Inline

```
result_status: OPEN
result_summary: Dashboard computes the number; a committed ratchet file was not verified this pass.
result_evidence_paths:
  - docs/progress/baseline_round_0.json
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Worth reconsidering when a committed ratchet_state.json records strict_coverage 0.899590 and apply-ratchet.sh blocks a lower bound.

---

### IDEA-0024 -- production spill receipt probe

| Field | Value |
|---|---|
| `experiment_id` | `IDEA-0024` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `idea-wizard-orchestrator` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CONF-0002` |

### Hypothesis

> An oversize `zsx exec` result spills to a content-addressed ref and the envelope nulls `visibleTokenCount`.

### Motivation

`[SPEC-RES-002]` is verified at type level; `[SPEC-HON-002]` is not. This is the live probe.

### Minimal Reproducer

```bash
# targeted; never cargo test --workspace
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  cargo test -p zerostack-harness --lib spec_oracle -- --nocapture
```

### Expected Signal

Either a production emission site or a documented "no production spill path yet" that upgrades CONF-0002 to CONFIRMED_GAP.

### Falsifiability Criteria

Production already emits the receipt (then write the verifier and close CONF-0002).

### One-Line Invocation

```bash
rg -n "result_finalization_receipt" crates --glob '!**/zerostack-harness/**'
```

### Results Inline

```
result_status: OPEN
result_summary: Probe not executed this pass.
result_evidence_paths: []
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Blocked until a hub production path emits result_finalization_receipt.v1 with null visibleTokenCount and status requires_tokenzero_certification on an uncertified spill; track as SPEC-HON-002.

---

### IDEA-0001 / IDEA-0003 / IDEA-0004 / IDEA-0005

These are the pass-11 shapes for SURF-0003 / SURF-0001 / SURF-0007 / SURF-0009. They are not separate open hypotheses (parent already `CONFIRMED_GAP`). Recorded in `round_0/ideas/idea_wizard_phase2.md` so they are not rediscovered.

### IDEA-0009 -- ABI digest mutation test

Parent CONF-0005. Same OPEN status; do not double-count. Invocation lives on CONF-0005.

### IDEA-0025 -- fixture TokenZero Exact accept

Parent CONF-0003. Same OPEN status; do not double-count.

---

## Advanced-methods / frontier-math (promoted)

Scored in `ZeroStack__gauntlet_workspace/round_0/ideas/advanced_methods.md` and `frontier_math.md`. Promoted entries that are not already cards above:

### ADV-0001 -- submodular dashboard close order

| Field | Value |
|---|---|
| `experiment_id` | `ADV-0001` |
| `pillar` | `surface` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `advanced-methods-miner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0015` |

### Hypothesis

> Closing the four missing rows plus `F-ABI-PROPTEST-ROUNDTRIP` and `F-STORE-ENSURE-LAYOUT` (in-hub, non-engine) raises strict coverage more per line of code than any engine-surface re-include.

### Motivation

Submodular set cover on the 244 weight units. Missing weights: fuzz 3 + miri 2 + fromstr 1 + negotiate 1 = 7 units. ABI proptest 4 + ensure_layout 1 = 5 more. Engine excluded rows are 3 units and are out of repo.

### Minimal Reproducer

```bash
python3 - <<'PY'
# units from the matrix; confirm arithmetic only
print('missing', 3+2+1+1, 'partial_in_hub', 4+1)
PY
```

### Expected Signal

Pass-11 ranked list matches this set. If a different in-hub row has more units for less work, refine the rank.

### Falsifiability Criteria

A partial row with more units is smaller to close (then rerank).

### One-Line Invocation

```bash
python3 scripts/check_feature_coverage_dashboard.py
```

### Results Inline

```
result_status: OPEN
result_summary: Used to rank pass 11. Not a product patch.
result_evidence_paths:
  - conformance/contracts/supported_surface_matrix.toml
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry condition not applicable -- the gain is structural, not numerical.

---

### ADV-0002 -- e-process on spec-source hash stability

| Field | Value |
|---|---|
| `experiment_id` | `ADV-0002` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `advanced-methods-miner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `CONF-0006` |

### Hypothesis

> Treating agents-law sha256 drift as a software e-process observation (`p0=1e-6, λ=0.9, α=0.001`) would have rejected certifying after one Phase 8 append -- the same decision preflight already made.

### Motivation

Frontier-math compilation: e-process is already wired for CAS digest / engines-do-not-import. Extending it to spec hashes is optional scaffolding, not a pass-11 must.

### Minimal Reproducer

Compare preflight red vs `eprocess` reject on a one-bit hash change.

### Expected Signal

Redundant with CONF-0006. Promote only if preflight is bypassed.

### Falsifiability Criteria

Preflight already fails closed on hash drift (it does). Then this is NO_EVIDENCE as a new detector -- still useful as defense in depth.

### One-Line Invocation

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness --lib eprocess -- --test-threads=1
```

### Results Inline

```
result_status: OPEN
result_summary: Conditional (score 6). Preflight already catches the drift. Do not build a second detector in pass 11.
result_evidence_paths:
  - crates/zerostack-harness/src/eprocess.rs
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Not worth retrying as a standalone patch.

---

### ADV-0003 -- Little's-law inflight cancel leak

| Field | Value |
|---|---|
| `experiment_id` | `ADV-0003` |
| `pillar` | `conformance` |
| `status` | `OPEN` |
| `created_at_utc` | `2026-08-17T16:00:00Z` |
| `created_by_agent` | `advanced-methods-miner` |
| `bead_id` | `N/A` |
| `parent_hypothesis_id` | `SURF-0010` |

### Hypothesis

> After cancel, MCP inflight count must return to 0 (Little's law: arrivals = departures). A leftover inflight is a leaked permit.

### Motivation

Recent commits (`82a8795`, `f046d4d`) already fix cancel/permit release. This experiment is a counter, not a rewrite.

### Minimal Reproducer

Cancel N in-flight `zero_execute` calls; assert inflight gauge is 0.

### Expected Signal

If true: a test can fail on leftover inflight. If the counter does not exist, spawn a refinement.

### Falsifiability Criteria

Inflight is already 0 after cancel in an existing test.

### One-Line Invocation

```bash
rg -n "inflight" crates/zero-mcp/src/mcp_transport.rs crates/zero-codemode/src
```

### Results Inline

```
result_status: OPEN
result_summary: Not run. Complements IDEA-0015.
result_evidence_paths: []
result_impact:
spawned_remediation_bead: N/A
spawned_experiments: []
closed_at_utc:
```

### Closure Predicate

Retry only if this workload class exhibits measurable inflight residue above 0 after cancel on a hub test outside rival-dirty fszero.rs.

---

## Idea-wizard raw list (index only)

30 candidates, top-5, and +10 are in:

`/Users/aditya/AI/ZeroStack__gauntlet_workspace/round_0/ideas/idea_wizard_phase2.md`

Duplicates of CONFIRMED_GAP cards are **not** extra OPEN hypotheses.

---

## Honesty notes

- No product remediations were implemented in Phase 10.
- Rival dirty files were not read for edits and must not be touched: `crates/zsx-core/src/fszero.rs`, `docs/codemode.md`, `tests/unit/zsx-core/fszero_tests.rs`, `.zsx_patch.diff`.
- Never `cargo test --workspace`.
- `CONFIRMED_GAP` rows already have evidence in the matrix, Phase 9 scorecard, or cited source. `OPEN` rows do not.
- Engine enforcement, TokenZero Exact emission, and Q99 engine accounting are out of this repo.
