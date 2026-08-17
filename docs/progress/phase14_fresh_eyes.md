# Phase 14 fresh-eyes

Prompts A, B, and C applied verbatim against gauntlet code from
`cf6ca4d` through `dd0730b` (plus `626145c`, `1087a55`).

Dashboard remains **red**. Partial is not rounded up.

## Findings

| Sev | File | Issue | Status |
|---|---|---|---|
| high | `crates/zerostack-harness/src/engine_identity.rs` | Public fields let `EngineIdentity::oracle("zerostack")` (or a struct literal) become a self-compare. `ExecutionEnvelope` never called the comparator guard. | **fixed** -- fields private; `oracle()` panics on subject/empty/disallowed labels; `assert_identities` + envelope construction |
| high | `crates/zerostack-harness/src/failure_bundle.rs` | `PartialOnDisk` omitted `/failure/first_divergence`. Test only grepped the jsonptr string (false green). | **fixed** -- fallback writes `failure`; tests parse the pointer |
| high | `crates/zerostack-harness/src/golden.rs` | `catalog_tag_count == 53` (and hardcoded 8 / 4) would fail a real bless after SPEC-TAGS/CONTRACT/fixture edit, and would *not* fail an unblessed catalog edit. | **fixed** -- compare dump counts to live `SPEC-TAGS.md` / CONTRACT.md / fixture |
| high | `scripts/check_golden_integrity.py` | Checksums/manifest could agree with themselves after a matrix/SPEC-TAGS edit if only on-disk goldens were hashed. Artifact could be missing from checksums. | **fixed** -- live recapture compare + checksums must list every manifest path |
| high | `tests/snapshots/golden_invariants__phase4_logical_counts.snap` | Insta pin still said present=66 / partial=8 after live dump + dashboard moved to 67 / 7. | **fixed** -- snapshot matches live histogram |
| med | `crates/zerostack-harness/src/measure.rs` | `iter >= MIN_ITERS` on a 0-based loop collected 4 samples, not 3. | **fixed** -- stop on `times.len() >= MIN_ITERS` |
| med | `scripts/run_miri_narrow.sh`, `scripts/bench/store_cas.py`, `.github/workflows/ci.yml` | Invented `CARGO_TARGET_DIR=/tmp/rch_target_zerostack` (AGENTS.md requires `$RCH_TARGET_BASE`). | **fixed** -- `${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack` |
| med | `docs/progress/surface-deferrals.md` | DEFERRED rows still claimed no fuzz targets / no FromStr / no `negotiate` after `626145c` marked those present. | **fixed** -- CLOSED superseding entries; open-candidates list updated |
| low | `crates/zerostack-harness/src/bin/oracle-preflight-doctor.rs` | Em dash in CLI output. | **fixed** -- `--` |
| low | `crates/zerostack-harness/tests/oracle_smoke.rs` | `preflight_does_not_panic_and_is_green` did not require green; `subject_equals_subject` was a tautology. | **fixed** -- renamed; subject vs oracle identity assert |
| -- | `crates/zsx-core/src/fszero.rs`, `docs/codemode.md`, `tests/unit/zsx-core/fszero_tests.rs` | Accidental `cargo fmt -p` reformatted rival-dirty files. Restored all *other* accidental fmt via stash. Pre-fmt rival bytes were not snapshotted. | **left** -- not staged, not committed |

## Checked and found correct

1. **artifact_id vs run_id.** `CanonicalEnvelope` omits `run_id`. Tests prove two run ids share one SHA-256 and a workload change does not.
2. **Partial is not present.** Dashboard `contribution("partial") = 0.5`; `family_verdict` never returns `full` when partial/missing/excluded remain. Gate stays red (`effective=0.952282`, `strict=0.940573`, `strict_100_certifiable=false`).
3. **ensure_layout** creates `engine_dir`, `blobs/`, and `gc/` under `cas_host()`. Tests cover CAS-host and local-unified.
4. **FromStr == parse.** `FromStr` delegates to `ZeroRefV1::parse`. API + proptest assert equality.
5. **negotiate** returns `IncompatibleVersion` on major mismatch; same major / any minor is accepted.
6. **measure_with_teardown** captures `start.elapsed()` before `teardown()`. 20 ms teardown sleep stays under a 15 ms median.
7. **Weight sum** is 1.0 globally (`1.000000000000`) via `math.fsum` / `SPEC-FU-003`.
8. **MCP late-Ok after cancel** is `commit_race`, not retryable, payload kept; late domain Err stays that Err. Inflight permit is released.
9. **Q99 empty window** is `unavailable`, `no_demand_observations`, no bare `%`. Residual engine accounting stays honest partial.
10. **Ledger retry phrases.** `check_ledger_retry.py` is green on the three durable ledgers.
11. **Bench-history** self-oracle: `cv_pct` null is noise, not a win; no invented keep.
12. **EngineIdentity allowed oracle set** is `{spec-v1, property-suite-v1, prior-commit-<sha>, round-trip, miri, clippy}`; subject label `zerostack` is not in that set.

## Prompt notes

- **A:** authoring-mode pass over harness + zero-ref + store layout + scripts.
- **B:** random walk into dashboard, catalog hashes, RCH target dirs, ledger honesty, CI comment.
- **C:** cross-agent drift: stale insta vs dashboard 67/7, PartialOnDisk vs matrix note, hardcoded 53 vs live SPEC-TAGS.

## Verification

```
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness -- --test-threads=1
# 35 lib + 5 crash + 6 golden + 9 smoke + 2 store_cas = ok

rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-ref -- --test-threads=1
# 8 api + 2 proptest + 1 doctest = ok

rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo clippy -p zerostack-harness --all-targets --no-deps -- -D warnings
# Finished, exit 0

python3 scripts/check_feature_universe_weights.py
# feature-universe ok: features=77 present=67 partial=7 missing=0 excluded=3 weight_sum=1.000000000000

python3 scripts/check_feature_coverage_dashboard.py
# gate=red effective=0.952282 strict=0.940573

python3 scripts/check_golden_integrity.py
# golden-integrity ok: checksums=12 artifacts=11 tier1=5 schema=1.0.0

python3 scripts/check_ledger_retry.py
# ledger-retry ok: files=3

python3 scripts/check_bench_history.py
# bench-history ok (cv_pct null is not a win)
```

## Files changed (this commit)

Harness identity / bundle / golden / measure / spec catalog parse; golden integrity recapture; RCH target-dir scripts; ledger honesty; insta pin; invariant catalog hashes.

Not committed: rival dirty `fszero.rs` / `codemode.md` / `fszero_tests.rs` / `.zsx_patch.diff`.
