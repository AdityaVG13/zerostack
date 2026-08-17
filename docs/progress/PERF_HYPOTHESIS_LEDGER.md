# Performance hypothesis ledger

Index into [GAUNTLET_EXPERIMENT_DESIGNS.md](GAUNTLET_EXPERIMENT_DESIGNS.md). Full template fields live there. Grep the negative ledger first: [perf-negative-results.md](perf-negative-results.md).

**Open-hypothesis count (this pillar):** 3 (`PERF-0007`, `OPEN-0015`, `OPEN-0016`).
**Confirmed gaps owed to pass 11:** 2 (`PERF-0001`, `PERF-0003`). `PERF-0003` is install-cass, not a product patch.

| ID | Status | One-line hypothesis | Invocation |
|---|---|---|---|
| CLOSED-0006 | CLOSED | Bench-history ratchet exists (`d6cf7ed`). | `python3 scripts/check_bench_history.py` |
| CLOSED-0011 | CLOSED | Test-profile `store_cas_microbench` is not a keep. | `cargo test -p zerostack-harness --test store_cas_microbench` |
| PERF-0001 | CONFIRMED_GAP | Seed `cv_pct` is null; `keep_eligible=false`; not a win. | `python3 scripts/check_bench_history.py` |
| PERF-0002 | CLOSED | Fat-LTO waiver decided; do not flip to thin. | `rg -n '\[profile.release-perf\]' Cargo.toml` |
| PERF-0003 | CONFIRMED_GAP | `cass` not on PATH; 60-day mine is blocked. | `command -v cass` |
| PERF-0007 | OPEN | `SharedCas::put` ≥0.1% self-time under release-perf. | `cargo test -p zerostack-harness --test store_cas_microbench` |
| OPEN-0015 | OPEN | Ten rch repeats yield `cv_pct <= 5`. | `python3 scripts/check_bench_history.py` |
| OPEN-0016 | OPEN | rch comprehensive-bench JSON v3 can replace the 12-row seed. | `cargo run -p zerostack-harness --bin comprehensive_bench -- --help` |

## Already correct (do not re-open)

- `release-perf` line tables + symbols (`d6cf7ed22618f9b6289745c822f935c3eb59b828`)
- `measure` teardown outside the timed window
- `savingsbytes-as-tokens` REJECTED
- `estimate-labeled-exact` REJECTED
- `test-profile-microbench-as-keep` REJECTED
- `comprehensive-bench-93-on-mac` DEFERRED (use OPEN-0016)

## Pass-11 note

No in-hub product hot-path change this round. Cass install and a replicated cv_pct run are infra/measurement. Profile-first (PERF-0007) is required before any store edit.
