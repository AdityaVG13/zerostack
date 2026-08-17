# Phase 15 -- narrow soak / deep-validation (round 0)

**Date:** 2026-08-17T14:00:03Z
**Target HEAD:** `969220b2f39c5c6e3fb033c29ac753fb758c8bea`
**Mode:** `gauntlet-greenfield` / Phase 15 narrow soak (not 24h, not multi-day)
**Class:** `Greenfield-Rust-class`
**Machine:** local `Adityas-MacBook-Pro-5.local`; cargo/clippy offloaded to rch worker `spark-1672`

This pass **ran** the honesty gates, targeted crate tests, host Miri,
30s fuzz smokes, and harness clippy. It did **not** run a 24h fuzz, a
multi-day Miri, loom/shuttle, BOCPD, or adversarial-search. Scorecard:
`docs/progress/soak_round_0.json`.

Never `cargo test --workspace`. Never a 24h claim.

## Headline numbers (truncate_score, 6 decimals)

| Metric | Value |
|---|---|
| FeatureUniverse | 77 features; present=67 partial=7 missing=0 excluded=3 |
| weight_sum | 1.000000 |
| effective_coverage (in-scope) | **0.952282** |
| strict_coverage (excluded is debt) | **0.940573** |
| dashboard release gate | **red** (honest; partial + excluded remain) |
| conformal / release decision | use **0.940573** lower/strict bound |
| bench-history | pass (self-oracle, no regression) |
| savings-bench primary_score | 0.044 (unchanged; lower_is_better) |
| cv_pct | **null** -- unknown; **not a win**; keep_eligible=false |
| zerostack-harness tests | 57 passed, 0 failed |
| zero-ref tests | 11 passed, 0 failed |
| zero-abi tests | 15 passed, 0 failed |
| zero-store tests | 2 passed, 0 failed |
| host Miri `-p zero-ref` | 11 passed, 0 failed (rch local fallback) |
| rch Miri | **not run** (not interceptable + worker missing `cargo-miri`) |
| fuzz `zeroref_parse` 30s | 28,941,563 execs; 0 crashes |
| fuzz `abi_frame_decode` 30s | 2,477,542 execs; 0 crashes |
| clippy `-p zerostack-harness -D warnings` | pass |
| TrueDivergence / FailureBundle | 0 / 0 |

## Campaign table

| Command | Where | Exit | Duration | Summary |
|---|---|---:|---:|---|
| `python3 scripts/check_feature_universe_weights.py` | local | 0 | (parallel batch) | features=77 present=67 partial=7 missing=0 excluded=3 weight_sum=1.000000000000 |
| `python3 scripts/check_feature_coverage_dashboard.py` | local | 0 | (parallel batch) | effective=0.952282 strict=0.940573 gate=red families=21 catalog=6 |
| `python3 scripts/check_golden_integrity.py` | local | 0 | (parallel batch) | checksums=12 artifacts=11 tier1=5 schema=1.0.0 |
| `python3 scripts/check_bench_history.py` | local | 0 | (parallel batch) | primary_score unchanged 0.044; cv_pct null; verdict pass |
| `python3 scripts/check_ledger_retry.py` | local | 0 | (parallel batch) | files=3 |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zerostack-harness -- --test-threads=1` | spark-1672 | 0 | 70.386s (remote exec 35.306s) | 35 lib + 0 bin + 5 crash + 6 golden + 9 oracle + 2 store_cas + 0 doctest = **57** |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-ref -- --test-threads=1` | spark-1672 | 0 | 57.966s (remote exec 23.810s) | 0 lib + 8 api + 2 proptest + 1 doctest = **11** |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-abi -- --test-threads=1` | spark-1672 | 0 | 67.505s first; 64.369s after unused_mut fix | 13 lib + 2 proptest = **15**; unused_mut gone on re-run |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-store -- --test-threads=1` | spark-1672 | 0 | 57.120s (remote exec 22.662s) | 0 lib + 2 ensure_layout = **2** |
| `scripts/run_miri_narrow.sh` | **host fallback** | 0 | 30.985s | rch WARN `non-compilation command`; miri on `aarch64-apple-darwin`: 8+2+1 green |
| `rch diagnose -- cargo +nightly miri test -p zero-ref` | local classify | n/a | -- | Kind=none; WOULD NOT INTERCEPT (`cargo subcommand not interceptable`) |
| `ssh spark-1672-rch-remap 'cargo +nightly miri --version'` | spark-1672 | 1 | -- | `cargo-miri` **not installed** for `nightly-aarch64-unknown-linux-gnu`; worker rustc 1.97.1 |
| `cargo +nightly fuzz run zeroref_parse -- -max_total_time=30` | host | 0 | 85.482s (compile ~54s + 31s run) | 28941563 execs; cov=215 ft=471 corp=200; 0 crashes |
| `cargo +nightly fuzz run abi_frame_decode -- -max_total_time=30` | host | 0 | 31.852s | 2477542 execs; cov=1493 ft=5335 corp=1552; 0 crashes |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo clippy -p zerostack-harness --all-targets --no-deps -- -D warnings` | spark-1672 | 0 | 53.392s (remote exec 22.201s) | Finished `dev` with `-D warnings` |

Mid-campaign rch retest of `zero-abi` during the live fuzz write exited **1** because rsync saw vanishing corpus files (`file has vanished`). Not a crate bug. Re-ran after fuzz stopped: exit 0.

## Host vs rch Miri (F-MIRI-NARROW)

| Probe | Result |
|---|---|
| Host `cargo +nightly miri --version` | `miri 0.1.0 (14210df0e2 2026-05-31)` |
| Host `scripts/run_miri_narrow.sh` | green; rch `exec` warned and ran locally; target dir `.../miri/aarch64-apple-darwin/...` |
| rch hook classification | `cargo +nightly miri test` is **not** a compilation command |
| Worker `spark-1672` | Linux aarch64; rustc 1.97.1; nightly present; **no** `cargo-miri` component |

Honest status after this pass: **still `partial`**. Host is green (already true in Phase 11). The feature retry_condition is `miri test -p zero-ref green on rch`. That did not happen. Matrix not flipped. Goldens not blessed. Dashboard stays red.

## Bugs found and fixed

1. **`unused_mut` in `crates/zero-abi/src/zerokernel.rs`** (`unknown_fields_fail_closed`). First `cargo test -p zero-abi` warned `variable does not need to be mutable` on `v2`. Removed `mut`. Re-run: 15 passed, warning gone. Not a TrueDivergence.

No other soak defect in gauntlet/harness/zero-ref/zero-abi/zero-store.

## Not claimed / not run

- 24h differential fuzz
- Multi-day Miri / loom / shuttle / crash-boundary / BOCPD
- `cargo test --workspace`
- Dashboard green (7 partial + 3 excluded remain)
- F-MIRI-NARROW present

Generated libFuzzer corpus (hex-named files) was dropped after the smoke. Named seeds `fuzz/corpus/zeroref_parse/valid_blob` and `fuzz/corpus/abi_frame_decode/handshake.json` kept. `.gitignore` now ignores `fuzz/corpus/*/[0-9a-f]*` so a live fuzz cannot race rch rsync.

## Remaining partials (unchanged)

`F-REF-ENGINE-ADOPTION-LOCKSTEP`, `F-CONF-HARNESS`, `F-CI-PR-GATES`, `F-MIRI-NARROW`, `F-REF-ERROR-TAXONOMY`, `F-STORE-QUARANTINE-REAP`, `F-ZSX-Q99-REPORT`.

Excluded (engine-only debt): `F-FSZERO-PRIVATE-ENGINE-SURFACE`, `F-GRAPHZERO-PRIVATE-ENGINE-SURFACE`, `F-TOKENZERO-PRIVATE-ENGINE-SURFACE`.

## Already correct (left alone)

1. FeatureUniverse `sum(weights) == 1.0` and required retry_condition on partial/missing/excluded
2. Dashboard gate **red** with `partial_never_rounds_up` and excluded-as-debt
3. Three-tier goldens + `check_golden_integrity.py` (never blesses)
4. Bench-history ratchet (`cv_pct` null is not a win)
5. Ledger retry-condition lint (three files)
6. EngineIdentity Subject≠Oracle + both-error comparator
7. Crash oracle + `store_cas_microbench` JSON v3 as a test, not a keep
8. `zero-ref` `#![forbid(unsafe_code)]` + host Miri green on the narrow crate
9. cargo-fuzz targets `zeroref_parse` + `abi_frame_decode` exist and smoke without crash
10. CONTRACT §8: no in-repo conformance CLI (`F-CONF-HARNESS` stays partial)

## Files changed this pass

- `crates/zero-abi/src/zerokernel.rs` -- drop unused `mut`
- `.gitignore` -- ignore generated hex fuzz corpus
- `docs/progress/SURFACE_PARITY_HYPOTHESIS_LEDGER.md` -- SURF-0002 evidence
- `docs/progress/phase15_soak.md` -- this file
- `docs/progress/soak_round_0.json` -- machine scorecard

Rival dirty files were not touched: `crates/zsx-core/src/fszero.rs`, `docs/codemode.md`, `tests/unit/zsx-core/fszero_tests.rs`, `.zsx_patch.diff`.
