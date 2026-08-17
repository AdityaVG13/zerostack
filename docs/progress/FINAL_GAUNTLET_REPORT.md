---
name: FINAL_GAUNTLET_REPORT
schema_version: gauntlet.final-report.v1
generated_at_utc: "2026-08-17T14:10:00Z"
run_id: "20260817-141000-d4caada"
port_name: "ZeroStack"
reference_name: "spec-as-oracle"
reference_version: "conformance/CONTRACT.md + docs/racc/ + FeatureUniverse"
project_class: "greenfield-rust"
final_artifact_tier: "public-release"
certification_verdict: "NOT CERTIFIED"
recommendation: "HOLD"
source_file_hashes:
  - path: "docs/progress/baseline_round_0.json"
    sha256: "2d8e6f4992241959b7f5a90e353ecf0a0198c608277a3832be80c3bd77bf91c7"
  - path: "docs/progress/soak_round_0.json"
    sha256: "7323130e1d30193d855ed739077e5547edebff136ded84833add274ec9e0484c"
  - path: "conformance/contracts/feature_coverage_dashboard.json"
    sha256: "02ca9ec72fc8d05def46ab4312c71061d899b60f800e5d9e57ad87870a6c645f"
  - path: "docs/progress/REMEDIATION_PLAN.md"
    sha256: "3274bb55de7b37bcf8622568acc530ececc581c6db99d866028ccc4497a98b8d"
  - path: "docs/progress/phase13_beads.md"
    sha256: "20505cd6f57ff3fab33c95923f3bc310d8430204faefe2eb0a345aafb04bdc10"
  - path: "docs/progress/phase15_soak.md"
    sha256: "69e0293cc755f5ab7b224902c5f3102136a8b2c938dcb3fcfd880d9113039523"
  - path: "docs/progress/GAUNTLET_EXPERIMENT_DESIGNS.md"
    sha256: "73cb1fae6a9dde8652da0addd587cf2d4f2390851d2929d4e67956aa4a5f1508"
  - path: ".bench-history/savings-bench.latest.json"
    sha256: "cfc3414f00637b1643c05e22b7df7d79c0815e02d8866815df2a3e3566c439de"
---

# FINAL_GAUNTLET_REPORT -- ZeroStack vs spec-as-oracle (public-release)

**NOT CERTIFIED.** Final-artifact tier is `public-release`. The `CERTIFICATION_*` 100% constants (`CERTIFICATION_MIN_VERIFICATION_PCT = 100.0`, `CERTIFICATION_REQUIRED_SUITE_PASS_RATE_PCT = 100.0`, `CERTIFICATION_MAX_HIGH_SEVERITY_COUNTEREXAMPLES = 0`, `CERTIFICATION_MAX_EVIDENCE_AGE_HOURS = 24`) are **not met**. No `certification_bundle/` was produced. No pillar is painted green.

Recommendation: **HOLD**.

HEAD at report time: `d4caada` (`phase15: narrow soak campaign`). This document is the Phase 16 public-release scorecard, not a release certificate.

## 1. Executive Summary

On **perf** the only committed baseline is the savings-bench self-oracle seed: `primary_score = 0.044` (lower_is_better token ratio, not a wall-clock speedup vs a reference), `cv_pct = null`, `keep_eligible = false`. There is no keep. Fat-LTO is waived by name. No conformal lower bound on a comprehensive-bench matrix exists because that matrix was never run.

On **conformance** the harness wires 48 spec verifiers; 5 tags stay `UNVERIFIED`. Preflight does **not** certify gitignored `AGENTS.md`. `FailureBundle` and the crash oracle are present. Zero `TrueDivergence` / `FailureBundle` emissions in the Phase 9 and Phase 15 scorecards. This is not a 100% suite-pass certification.

On **surface**, FeatureUniverse has 77 rows: present=67, partial=7, missing=0, excluded=3. Effective coverage `0.952282`. Strict coverage (excluded is debt) `0.940573`. Dashboard **gate=red**. Partial never rounds up.

Convergence of the FrankenSQLite 10-round Phase 5-10 comprehensive-bench loop was **not** reached. This skill-loop did **16 phase passes** (one pass per gauntlet phase, greenfield resume). Two consecutive clean gauntlet rounds were **not** achieved. `cass` is missing. rch Miri is missing.

## 2. Per-Pillar Status

Headline (do not invert): **perf = no keep / not green**; **conformance = harness present, not certified / not green**; **surface = gate red / not green**.

### 2a. Performance -- no keep

| Category | Score | cv_pct | Ratio vs reference | Primary MT8 frame attribution |
|---|---|---|---|---|
| `exact_tokens` (savings-bench seed) | `0.044000` | **null** | n/a (self-oracle; no reference impl) | none -- profile-first never run |
| `ReadSingle` / `ReadAggregate` / `Write*` / `ConcurrentWriters` / `MixedOltp` | **not measured** | -- | -- | -- |

Source: `.bench-history/savings-bench.latest.json` (`schema_version = zerostack.comprehensive-bench-report.v3`, `seed_kind = self-oracle-prior-commit`, `keep_eligible = false`).

Pass-over-pass (Phase 16 honesty re-run of `python3 scripts/check_bench_history.py`):

- primary score: unchanged (`0.044`)
- geomean: absent in both files; not gated
- p90: absent; not gated
- throughput: absent; not gated
- `cv_pct` is null (unknown; **not a win**)
- verdict: pass (no regression). within-noise / unknown-cv is **not** a win

Keep-gate status:

| Rule | Status |
|---|---|
| Profile-first ≥0.1% self-time | **not met** |
| Both gates same run window | **not met** (no focused+broad pair) |
| `release-perf` with measured `cv_pct` | seed `cargo_profile = not-recorded` |
| `cv_pct` reported and `<= 5` | **null** -- ineligible |
| MT8 attribution | **none** |
| Fat-LTO | **waived by name** in `Cargo.toml` (`lto = "fat"` shared with `release-node`; not a silent lying profile) |
| comprehensive-bench 93-scenario matrix | **not run** (deferred: `comprehensive-bench-93-on-mac`) |

This pillar is **not green**. A null `cv_pct` is unknown, not a win.

### 2b. Conformance -- harness present, not certified

| Behavior class | Oracle pass | FailureBundle count | Distinct MismatchSignatures | E-process e-value |
|---|---|---|---|---|
| Spec verifiers (wired) | 48 structural/type-level | 0 emitted (Phase 9 + 15) | n/a | preflight hash is advisory, not a second e-process |
| UNVERIFIED tags | 5 remain | -- | -- | -- |
| Crash oracle (`crash_oracle` test) | 5/5 (Phase 15: harness 57 passed) | 0 TrueDivergence | -- | -- |
| Golden three-tier | checksums=12 artifacts=11 tier1=5 schema=1.0.0 | -- | -- | -- |
| Preflight `AGENTS.md` | **does not certify** gitignored law | -- | -- | -- |

Five UNVERIFIED SPEC tags (`docs/spec/SPEC-TAGS.md`; `docs/progress/CONFORMANCE_HYPOTHESIS_LEDGER.md`):

| Tag | Why unverified | Card |
|---|---|---|
| `[SPEC-COMP-002]` | journal+undo types exist; no live later-failed-step execute | CONF-0001 |
| `[SPEC-HON-002]` | hub crates do not emit `result_finalization_receipt.v1` (fixtures only) | CONF-0002 |
| `[SPEC-HON-006]` | permission, TokenZero-owned Exact | CONF-0003 |
| `[SPEC-HUB-002]` | global fail-loud; no single static surface is a complete proof | CONF-0004 |
| `[SPEC-HUB-005]` | digest bump needs a mutation test | CONF-0005 |

Preflight (Phase 9 scorecard, then Phase 11 CONF-0006): gitignored `AGENTS.md` is **not** a certifying pin. Phase 9 first saw hash drift (`expected da53da83…` / `got 7211bb0e…`) and recorded it; Phase 11 dropped `agents-law` from the certifying set (advisory yellow only). That is honesty, not a green preflight certificate.

Conformal lower bound on a full parity suite: **not computed**. There is no `reports/ratchet_state.json` and no 93-scenario / full-oracle conformal band. Release decision on surface uses the **strict** score `0.940573`, not the effective `0.952282`.

This pillar is **not green**.

### 2c. Surface Parity -- gate red

Re-run 2026-08-17 (`python3 scripts/check_feature_coverage_dashboard.py`):

```
feature-coverage-dashboard ok: features=77 present=67 partial=7 missing=0 excluded=3
weight_sum=1.000000000000 effective=0.952282 strict=0.940573 gate=red
families=21 catalog=6 statuses={'present': 67, 'partial': 7, 'excluded': 3}
```

| FeatureUniverse family | Present | Partial | Missing | Excluded | Weighted | Strict | Verdict |
|---|---:|---:|---:|---:|---:|---:|---|
| ci | 0 | 1 | 0 | 0 | 0.500000 | 0.500000 | partial |
| conformance | 1 | 1 | 0 | 0 | 0.687499 | 0.687499 | partial |
| engine-scope | 0 | 0 | 0 | 3 | 0.000000 | 0.000000 | none |
| gauntlet-oracle | 2 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| hub-policy | 3 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| machine-permit | 2 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| quality | 1 | 1 | 0 | 0 | 0.800000 | 0.800000 | partial |
| surface | 1 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-abi | 11 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-cert | 2 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-codemode | 3 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-gate | 3 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-gauge | 1 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-ledger | 2 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-mcp | 2 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-process | 2 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zero-ref | 7 | 2 | 0 | 0 | 0.881578 | 0.881578 | partial |
| zero-store | 13 | 1 | 0 | 0 | 0.989583 | 0.989583 | partial |
| zsx | 3 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| zsx-core | 7 | 1 | 0 | 0 | 0.961538 | 0.961538 | partial |
| zsx-node | 1 | 0 | 0 | 0 | 1.000000 | 1.000000 | full |
| **global** | **67** | **7** | **0** | **3** | **0.952282** | **0.940573** | **red** |

Coverage debt: excluded still counts for a strict-100% claim. `strict_100_certifiable = false`. Partial contribution is 0.5 and **never rounds up**.

Seven remaining partials (`soak_round_0.json#/feature_universe/partial_ids`):

`F-REF-ENGINE-ADOPTION-LOCKSTEP`, `F-CONF-HARNESS`, `F-CI-PR-GATES`, `F-MIRI-NARROW`, `F-REF-ERROR-TAXONOMY`, `F-STORE-QUARANTINE-REAP`, `F-ZSX-Q99-REPORT`.

Three excluded (engine-only debt):

`F-FSZERO-PRIVATE-ENGINE-SURFACE`, `F-GRAPHZERO-PRIVATE-ENGINE-SURFACE`, `F-TOKENZERO-PRIVATE-ENGINE-SURFACE`.

This pillar is **not green**. Dashboard red is honest.

## 3. Findings Table (severity-ranked)

| Severity | Pillar | Finding ID | Description | Evidence path | Remediation bead |
|---|---|---|---|---|---|
| HIGH | surface | SURF-0006 / F-CI-PR-GATES | CI is `workflow_dispatch` only; no PR/push cargo test; `dsr quality --tool zerostack` has zero checks | `.github/workflows/ci.yml`; `OPEN-0014` | `zerostack-gauntlet-surf-0006-1nlp` |
| HIGH | perf | PERF-0001 | savings-bench `cv_pct` is null; seed is not keep-eligible | `.bench-history/savings-bench.latest.json` | `zerostack-gauntlet-perf-0001-fq2l` |
| HIGH | perf | PERF-0003 | `cass` not on PATH; 60-day mine is a recorded blocker, not a skip | `docs/progress/perf-negative-results.md` `cass-unavailable-phase8` | `zerostack-gauntlet-perf-0003-049q` |
| HIGH | surface | SURF-0015 | release gate red: effective `0.952282` / strict `0.940573` | `conformance/contracts/feature_coverage_dashboard.json` | (gate is honest; close rows, do not flip) |
| HIGH | conformance | CONF-0001..0005 | 5 UNVERIFIED SPEC tags | `docs/spec/SPEC-TAGS.md` | `zerostack-gauntlet-conf-0005-pvmp` (HUB-005 only) |
| HIGH | quality | F-MIRI-NARROW | host Miri green; **rch Miri missing** (`cargo-miri` not on spark-1672 nightly; rch does not intercept) | `docs/progress/soak_round_0.json#/gates/miri_narrow` | `zerostack-gauntlet-miri-narrow-hxsh` |
| MEDIUM | surface | SURF-0013 | `quarantine_object` / `reap_stale_temps` exist; tests not inventoried | matrix `F-STORE-QUARANTINE-REAP` | `zerostack-gauntlet-surf-0013-7yad` |
| MEDIUM | surface | SURF-0011 | Q99 empty-window is `unavailable` in hub; engine `WorkerTokenAccountingV1` residual | matrix `F-ZSX-Q99-REPORT` | `zerostack-gauntlet-zsx-q99-v39v` (stay honest partial) |
| MEDIUM | surface | SURF-0012 | reserved `ZeroRefErrorClass` variants documented, not constructed | matrix `F-REF-ERROR-TAXONOMY` | `zerostack-gauntlet-ref-error-taxonomy-u74o` |
| MEDIUM | conformance | SURF-0005 | harness is a library; CONTRACT §8 forbids an in-repo conformance CLI | `conformance/CONTRACT.md` §8 | `zerostack-gauntlet-conf-harness-eh8k` |
| MEDIUM | surface | SURF-0008 | ClampEnd lockstep cannot be hub-enforced | matrix `F-REF-ENGINE-ADOPTION-LOCKSTEP` | not beaded (out-of-repo) |
| MEDIUM | all | rival-dirty-tree | four dirty paths exist and were **not** committed | `crates/zsx-core/src/fszero.rs`, `docs/codemode.md`, `tests/unit/zsx-core/fszero_tests.rs`, `.zsx_patch.diff` | track as `rival-dirty-tree` |
| MEDIUM | perf | OPEN-0016 | no comprehensive-bench 93-row matrix; 12-row seed only | `perf-negative-results.md` `comprehensive-bench-93-on-mac` | not a Mac job |
| MEDIUM | soak | Phase 15 narrow | 30s fuzz / host Miri; **not** 24h fuzz, multi-day Miri, loom, shuttle, BOCPD | `docs/progress/phase15_soak.md` | -- |
| LOW | perf | PERF-0002 | fat-LTO waived by name vs gauntlet thin-LTO template | `Cargo.toml` `[profile.release-perf]` | standing waiver |
| LOW | conformance | IDEA-0002 | tracked `AGENTS-MANDATE.md` is not the certifying agents-law pin | `docs/progress/AGENTS-MANDATE.md` | later, scored 4.5 |
| LOW | surface | IDEA-0017 | no committed `reports/ratchet_state.json` floor at `0.940573` | dashboard JSON | later, scored 4.5 |

Phase 15 found and **fixed** `unused_mut` in `crates/zero-abi/src/zerokernel.rs` (compiler warning, not a TrueDivergence). That is already closed on `d4caada`.

## 4. Per-Pillar Remediation Plan

Mined from `docs/progress/REMEDIATION_PLAN.md` and `docs/progress/phase13_beads.md`. Scores are `Impact × Confidence / Effort`. Gate: keep proposals with score ≥ 2.0. Do not implement new harness features in this Phase 16 pass.

### 4a. Performance

- **PERF-0001** -- standing rewrite C: leave the seed as the honest ineligible baseline (score **10.0**). Runner-up A: ten rch `release-perf` repeats for a numeric `cv_pct` (score **3.0**, later; bead `zerostack-gauntlet-perf-0001-fq2l`). Rejected B: invent `cv_pct` from 12 rows (score **1.0**, dishonest).
- **PERF-0003** -- standing rewrite B: recorded cass blocker + git-log fallback (score **10.0**). Runner-up A: install cass and mine 60d (score **7.5**, operator; bead `zerostack-gauntlet-perf-0003-049q`). Rejected C: fabricate cass hits.
- **PERF-0002** -- fat-LTO stays; named waiver. Not a keep.
- **OPEN-0016 / PERF-0007** -- rch comprehensive-bench and profile-first `SharedCas::put` later; both at the 2.0 floor. Do not edit CAS from a test-profile hunch.

### 4b. Conformance

- **CONF-0005** -- rewrite A: mutate a C-23/24/26 pin and fail if ABI digest is unchanged (score **6.0**; bead `zerostack-gauntlet-conf-0005-pvmp`). Later.
- **CONF-0001 / 0002 / 0003 / 0004** -- stay UNVERIFIED until live verifiers exist. Do not emit Exact from the hub (`[SPEC-HON-001]`). Do not invent a conformance CLI (CONTRACT §8; SURF-0005 standing score **10.0**).
- **CONF-0006** -- already CLOSED: gitignored `AGENTS.md` is not a certifying pin.

### 4c. Surface

- **SURF-0006** -- rewrite A (docs) applied; rewrite B (`dsr quality` targeted `-p` tests, score **8.0**) is the honest close. Rewrite C (GH `cargo test` job) is **policy-rejected** even though the numeric score is 10.0. Bead `zerostack-gauntlet-surf-0006-1nlp`.
- **SURF-0013** -- unit test put-reaps-stale-tmp + quarantine digest mismatch (score **6.0**; bead `zerostack-gauntlet-surf-0013-7yad`).
- **F-MIRI-NARROW** -- `miri test -p zero-ref` green **on rch** (score **4.5**). Host green is not the feature. Bead `zerostack-gauntlet-miri-narrow-hxsh`.
- **F-CONF-HARNESS / F-REF-ERROR-TAXONOMY / F-ZSX-Q99-REPORT** -- stay honest partials (standing scores 10.0). Beads exist to *document* that, not to paint present.

## 5. Unresolved-But-Explicitly-Deferred List

Forbidden-phrase check: predicates below are copied from the durable ledgers / matrix. Lint: `python3 scripts/check_ledger_retry.py` → `ledger-retry ok: files=3` (Phase 16 re-run).

| Pillar | Item | Retry-Condition Predicate (verbatim) |
|---|---|---|
| perf | `cass-unavailable-phase8` | "Blocked until cass lands on PATH (`command -v cass` succeeds and `cass health --robot` is green); track as cass-on-path." |
| perf | `cv-pct-null-savings-baseline` | "Retry only if this workload class exhibits measurable cv_pct below 5.0 on a replicated `release-perf` savings-bench run with keep_eligible true." |
| perf | `fat-lto-to-thin-switch` | "Worth reconsidering when the release-node profile family also leaves fat LTO, or when a dedicated rch release-perf flamegraph run shows missing frames under fat LTO." |
| perf | `comprehensive-bench-93-on-mac` | "Blocked until an rch-offloaded comprehensive-bench JSON v3 with cv_pct at or below 5 and a committed `.bench-history` refresh lands; track as F-STORE-BENCH-HISTORY." |
| perf | `test-profile-microbench-as-keep` | "Retry only if a profiler attributes a clearly-above-noise share to SharedCas::put or atomic_write_file on a release-perf store_cas workload with cv_pct at or below 5." |
| conformance | `f-conf-harness-stays-partial` | "Worth reconsidering when conformance/CONTRACT.md §8 is amended to allow an in-repo conformance CLI and F-CONF-HARNESS status is rewritten to present." |
| conformance | `spec-comp-002-unverified` | "Blocked until a hub verifier drives a multi-mutation execute whose subsequent step fails and proves earlier mutation rolled back; track as SPEC-COMP-002." |
| conformance | `spec-hon-002-unverified` | "Blocked until a hub production path emits result_finalization_receipt.v1 with null visibleTokenCount and status requires_tokenzero_certification on an uncertified spill; track as SPEC-HON-002." |
| conformance | `spec-hon-006-unverified` | "Blocked until a TokenZero certification receipt is accepted by the hub without rejection and a verifier asserts that permission; track as SPEC-HON-006." |
| conformance | `spec-hub-002-unverified` | "Blocked until a fail-loud suite covers silent-success and heuristic-labeled-exact on live zsx receipts (not comments); track as SPEC-HUB-002." |
| conformance | `spec-hub-005-unverified` | "Blocked until a mutation test changes a C-23/24/26 pin and fails if the ABI digest does not bump; track as SPEC-HUB-005." |
| surface | `F-FSZERO-PRIVATE-ENGINE-SURFACE` | "Worth reconsidering when FSZero is a public Cargo workspace member of this hub with its own CONTRACT.md." |
| surface | `F-GRAPHZERO-PRIVATE-ENGINE-SURFACE` | "Worth reconsidering when GraphZero is a public Cargo workspace member of this hub with its own CONTRACT.md." |
| surface | `F-TOKENZERO-PRIVATE-ENGINE-SURFACE` | "Worth reconsidering when TokenZero is a public Cargo workspace member of this hub with its own CONTRACT.md." |
| surface | `F-CI-PR-GATES-no-tests` | "Worth reconsidering when .github/workflows/ci.yml or DSR runs cargo test on every PR/push, not only workflow_dispatch fmt/clippy/build." |
| surface | `F-MIRI-NARROW` (matrix) | "miri test -p zero-ref green on rch" |
| surface | `F-STORE-QUARANTINE-REAP` | "Retry only when CAS put/gc actually moves corrupt objects into CAS_QUARANTINE_DIR and reaps temps older than CAS_TEMP_REAP_AGE, with a unit test covering both constants." |
| surface | `F-ZSX-Q99-REPORT` | "Retry only when FSZero/GraphZero adapters supply measured WorkerTokenAccountingV1 so L1/L2 Q99 windows are non-empty in production." |
| surface | `F-REF-ERROR-TAXONOMY` | "Retry only when store/resolution production paths in this crate construct Missing, Io, PolicyDenied, and LegacyAmbiguity, or those variants are removed from ALL." |
| surface | `F-REF-ENGINE-ADOPTION-LOCKSTEP` | "Retry only when a hub gate fails a sibling engine that uses Reject instead of ClampEnd, evidenced by a live zsx receipt or an engine CI job — not a comment-only lockstep claim." |
| surface | `dashboard-gate-red` | "Worth reconsidering when scripts/check_feature_coverage_dashboard.py reports gate=green and strict_100_certifiable=true." |
| surface | `rival-dirty-tree` | "Reconsider only inside the broader one-repo FSZero adapter cutover (track as rival-dirty-tree)." |

## 6. Convergence Evidence Appendix

**Honesty first.** The skill's convergence rule requires **all three**:

1. ≥10 full iterations of **Phases 5→10** (comprehensive-bench + conformance + surface re-baseline + idea-wizard).
2. Two consecutive **clean** rounds (`<3` new genuine findings each).
3. Every open hypothesis resolved (`OPEN` count = 0).

**None of those three hold.**

This loop did **16 skill-loop passes** (Phase 0 through Phase 16, one pass each, `gauntlet-greenfield` resume). That is **not** 10 rounds of Phase 5-10 comprehensive-bench. Two consecutive clean gauntlet rounds were **not** achieved. Open hypotheses remain: **15 OPEN** + **3 CONFIRMED_GAP** + **1 NO_EVIDENCE** (`GAUNTLET_EXPERIMENT_DESIGNS.md` census after Phase 12; still current at Phase 16).

| Skill-loop pass | Phase | What happened | New genuine findings (honest, not a tracker JSON) | Open after |
|---:|---|---|---|---|
| 1 | 0+1 bootstrap/recon | sibling workspace; 3775 pub items; 12 missing retry_condition | many (inventory) | n/a |
| 2 | 2 spec pin | 38→71 FeatureUniverse rows; weight_sum=1.0 | surface honesty | n/a |
| 3 | 3 oracle | EngineIdentity; 48 spec verifiers | oracle kernel landed | n/a |
| 4 | 4 golden | 5+5+1 three-tier; bless refuses without operator flag | golden schema | n/a |
| 5 | 5 perf harness | bench-history seed; `cv_pct` null; fat-LTO waived | no keep claimed | n/a |
| 6 | 6 conformance harness | FailureBundle + crash oracle + metamorphic | harness present | n/a |
| 7 | 7 surface | 71→77 rows; dashboard **red** 0.910788 / 0.899590 | gate invented, stays red | n/a |
| 8 | 8 ledgers | 41 entries; **cass blocker recorded** | cass missing | n/a |
| 9 | 9 baseline | gates run; preflight red on gitignored AGENTS.md; harness 53 | 1 recorded (hash drift, not blessed) | n/a |
| 10 | 10 idea-wizard | 49 cards (then 16 CLOSED / 13 CONFIRMED_GAP / 20 OPEN) | designs, not closures | 20 OPEN |
| 11 | 11 iterate | FromStr, fuzz, ABI proptest, ensure_layout, negotiate; missing 4→0 | remediations; miri stays partial | 5 CONFIRMED_GAP ranked leftover |
| 12 | 12 remediation | cancel + Q99 tests; present=67; **3 CONFIRMED_GAP remain** | AUTO-FIX only | 15 OPEN + 3 GAP |
| 13 | 13 beads | 32 beads; cycles=0; none claimed | handoff only | same |
| 14 | 14 fresh-eyes | 5 high-severity harness holes **fixed**; rival dirty expanded by fmt, **not committed** | **≥5 new genuine** (EngineIdentity privacy, FailureBundle fallback, golden live-count, integrity recapture, insta drift) | same |
| 15 | 15 soak | 30s fuzz 0 crashes; host Miri green; **rch Miri missing**; unused_mut fixed | 1 compiler warning + confirmed rch-miri hole | same |
| 16 | 16 artifacts | this report + runbook; honesty scripts re-run | **0 product fixes** (docs only) | **still 15 OPEN + 3 CONFIRMED_GAP** |

Exit conditions:

- ❌ ≥10 rounds of Phases 5–10 comprehensive-bench (`round_count` of that loop = **0**; skill-loop passes = 16)
- ❌ Last 2 rounds `<3` new genuine findings (pass 14 had ≥5; pass 15 had the rch-miri confirmation + unused_mut)
- ❌ Every open hypothesis resolved (15 OPEN remain)

There is no `reports/convergence_tracker.json` from a 10-round bench loop because that loop was never run. Do not invent one.

## 7. Certification Bundle Manifest

**Not produced.** Tier is `public-release`, not `certification-bundle`.

| Required by skill for certification-bundle | Status here |
|---|---|
| `certification_bundle/confidence_gate.json` | **absent** |
| `verification_contract.json` | **absent** |
| `release_certificate.json` (`strict-conformant-release.v1`) | **absent** |
| `ci_artifact_manifest.json` | **absent** |
| `benchmark_summary.json` | **absent** (seed only: `.bench-history/savings-bench.latest.json`) |
| `scorecards.json` | **absent** as a bundle file; Phase 9/15 JSON live under `docs/progress/` |
| `critical_path_report.json` | **absent** |
| `ratchet_state.json` | **absent** |
| `BUNDLE_MANIFEST.json` | **absent** |
| `RELEASE_CERTIFICATION_TEMPLATE.md` | **not written** (would imply a certification path this run does not have) |

Why certification is blocked (any one is enough):

1. `strict_coverage = 0.940573` ≠ 100%; `strict_100_certifiable = false`; gate **red**.
2. `CERTIFICATION_MIN_VERIFICATION_PCT = 100.0` fails (5 UNVERIFIED tags).
3. `cv_pct` null; no keep-eligible bench; no conformal perf lower bound.
4. Convergence rule failed (section 6).
5. `cass` missing; rch Miri missing.
6. Evidence age / soak: Phase 15 was a **narrow** soak (30s fuzz), not 24h.

`bundle_root_sha256`: **n/a** -- no bundle.

## 8. Negative-Ledger Summary

| Ledger | Path | Role |
|---|---|---|
| Perf | `docs/progress/perf-negative-results.md` | rejected / deferred perf ideas |
| Conformance | `docs/progress/conformance-negative-results.md` | rejected / deferred conformance hypotheses |
| Surface | `docs/progress/surface-deferrals.md` | excluded / deferred surface rows |

Phase 16 `python3 scripts/check_ledger_retry.py` → `ledger-retry ok: files=3`.

Approximate durable-entry counts (Phase 8 seed + later CLOSED superseding rows): perf ~12, conformance ~15, surface ~18. Cass mine across all three is the **same blocker** (`command -v cass` failed on this host at Phase 8 and again at Phase 16).

Top retry-condition forms actually used:

1. `"Not worth retrying as a standalone patch."` -- `savingsbytes-as-tokens`, `estimate-labeled-exact`, `daemon-install`, `commit-race-mislabel`, `engine-import-cycle`, `host-path-leak`, `both-error-as-failure`, `agents-md-as-feature-evidence`
2. `"Blocked until …; track as …."` -- cass, UNVERIFIED SPEC tags, historical fuzz/FromStr/negotiate (those three later CLOSED when the code landed)
3. `"Worth reconsidering when <GATE> crosses <THRESHOLD>."` -- dashboard green, CI tests, engine workspace membership, fat-LTO
4. `"Retry only if this workload class exhibits measurable cv_pct below 5.0 …"` -- savings-bench seed
5. `"Retry condition not applicable -- the gain is structural, not numerical."` -- EngineIdentity, landed FromStr/fuzz/negotiate, weight waiver

Patterns definitively retired (do not re-open):

- `self-compare-oracle-identity` -- CLOSED pass 3; Phase 14 then **hardened** privacy (public fields could still smuggle a self-compare)
- `invent-second-conformance-catalog` -- permanent reject unless CONTRACT §8 changes
- `release-perf-lying-profile` -- CLOSED pass 5 (line tables + symbols)
- `ten-crates-absent-from-feature-universe` -- CLOSED pass 2
- `F-FUZZ` / `F-REF-SERDE-FROMSTR` / `F-REF-CAPABILITY-NEGOTIATION` Pass-7 "missing" rows -- superseded CLOSED after Phase 11
- `both-error-as-failure` -- REJECTED; both-error is agreement regardless of message

## 9. Open Questions for the Maintainer

- **Public-release vs certify.** This report is the honest public scorecard. Certifying would require closing the 7 partials (or documenting them as `n/a` with a contract change), moving engines into the workspace or accepting excluded-as-debt forever, a keep-eligible `cv_pct<=5` bench, cass on PATH, rch Miri, and a real 10-round Phase 5-10 loop. None of that is implied by shipping these two markdown files.
- **DSR vs GitHub Actions.** The honest close of `F-CI-PR-GATES` is `dsr quality` targeted `cargo test -p`, not a GH `cargo test --workspace` job. Numeric score of the GH option is not permission.
- **Rival dirty tree.** Four paths are dirty and were not committed (Phase 14 even expanded `fszero_tests.rs` via accidental `cargo fmt`). A later cutover bead owns them. Do not "clean up" from a gauntlet pass.
- **Q99 / Exact / ClampEnd.** These residuals live in sibling engines. Hub comments claiming lockstep would be a lie.
- **Ratchet floor.** If you commit `reports/ratchet_state.json`, the floor must start at **`0.940573`** (Phase 12/15), not the Phase 9 `0.899590`.

## 10. Phase 16 honesty re-run (five Python scripts)

Ran locally from `/Users/aditya/AI/ZeroStack` on 2026-08-17. Full output:

```
feature-universe ok: features=77 present=67 partial=7 missing=0 excluded=3 weight_sum=1.000000000000
EXIT_WEIGHTS=0
feature-coverage-dashboard ok: features=77 present=67 partial=7 missing=0 excluded=3 weight_sum=1.000000000000 effective=0.952282 strict=0.940573 gate=red families=21 catalog=6 statuses={'present': 67, 'partial': 7, 'excluded': 3}
EXIT_DASH=0
golden-integrity ok: checksums=12 artifacts=11 tier1=5 schema=1.0.0
EXIT_GOLDEN=0
bench-history: primary_score: unchanged
bench-history: geomean: absent in both files; not gated
bench-history: p90: absent in both files; not gated
bench-history: throughput: absent in both files; not gated
bench-history: category:exact_tokens: unchanged
bench-history: cv_pct is null (unknown; not a win)
bench-history: verdict: pass (no regression). within-noise / unknown-cv is not a win
bench-history ok: bench=savings-bench baseline=.bench-history/savings-bench.latest.json current=.bench-history/savings-bench.latest.json
EXIT_BENCH=0
ledger-retry ok: files=3
EXIT_LEDGER=0
bench-history self-test ok
EXIT_BENCH_SELF=0
CASS_MISSING
```

`command -v cass` failed. `command -v rch` succeeded (`/Users/aditya/.local/bin/rch`). No workspace `cargo test --workspace` was run. Rival dirty files were not touched.

## What this report refuses to claim

- That any of the three pillars is green.
- That the port is certified, or that `CERTIFICATION_* = 100` holds.
- That savings-bench `0.044` is a keep or a speedup vs a reference.
- That `cv_pct` is known, or that `cv_pct > 5` could be ignored.
- That host Miri is rch Miri.
- That 30s fuzz is a 24h soak.
- That 16 skill-loop passes are 10 comprehensive-bench rounds.
- That two consecutive clean gauntlet rounds happened.
- That cass was mined (it was not; the blocker is recorded).
- That the rival dirty tree was cleaned or committed.
- That preflight certifies gitignored `AGENTS.md`.
- That excluded rows can be dropped from a strict-100% claim.
- That partials may be rounded up to present.

---

*Generated by the running-the-gauntlet-on-your-rust-port skill at Phase 16. Tier: public-release. To keep the port honest after this snapshot: `docs/progress/PARITY_RUNBOOK.md`.*
