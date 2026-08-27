# Performance budgets (project contract)

Single human/agent source of truth for **northstar** latency and size budgets.
Per-scenario measurement cards live under `benchmarks/*.md` and
`benchmarks/perf_harness.md`. Statistical hygiene and profilable build rules:
[`benchmark-integrity.md`](benchmark-integrity.md). Profiling how-to:
[`profiling.md`](profiling.md).

This file is the committed contract. Local gitignored DEFINE cards under
`tests/artifacts/perf/` may hold machine-specific fingerprints; they must not
be the only place budgets exist.

## Host class and provenance

Absolute walls are meaningful only for a declared host class (see integrity
policy Environment fingerprint). Historical northstar samples on this program
used **Apple M5 Max**-class hosts with `cargo_profile=release-perf`. Do not
compare absolute walls across host classes without recalibration.
Named classes and CI stamping: [benchmark-integrity Host class identity](benchmark-integrity.md#host-class-identity-ci-vs-published-local) (`fszero-twit`).

## Variance envelope (suite-wide)

Applies to same-host, same-workload p95 drift vs the previous accepted baseline:

| Drift | Action |
| :-- | :-- |
| ≤ 10% | Noise envelope -- no improvement or regression claim |
| > 10% | Investigate (raw vectors + fingerprints) |
| > 20%, or three consecutive same-direction envelope breaches | Block re-baseline; file an issue |

Enforcement tooling: `scripts/variance_envelope.py` (p95 drift vs multi-run or
`--baseline`) and optional `scripts/bench_baseline.sh` (hyperfine +
`env_fingerprint.py`). See also `fszero-tep8` for NCIB surface ratchet only.

## Status legend

| Status | Meaning |
| :-- | :-- |
| **gated** | Runner or test fails the process on miss |
| **aspirational** | Product target; measured and published; **not** silent-green CI |
| **observational** | Fingerprint / research only; no product pass-fail |

## Northstar surfaces

### Index cold / warm (10k files)

| Metric | Budget | Status | Notes / provenance |
| :-- | :-- | :-- | :-- |
| Cold start p95 wall (10k files) | ≤ 5 s | **aspirational** | Local DEFINE `tests/artifacts/perf/20260704_001/DEFINE.md` recorded ~11.7 s (≈2.3× over). Do not CI-green this until measured under target. |
| Warm start p95 wall (10k files) | ≤ 1 s | **aspirational** | Same DEFINE: ~4.4 s (≈4.4× over) at capture time. |
| Cold 100k wall | (no committed ≤ target) | **aspirational backlog** | Known over-target historically; any published claim must stay honest (no silent pass). Coord CI ratchets: `fszero-tep8`. |

Scenario cards / runners: cold/warm index paths via `FSZERO_STARTUP_INDEX=1` and
DEFINE-linked scripts under `tests/artifacts/perf/` (local) plus
`benchmarks/index-scaling.md` / `ingest-scaling.md` (observational speedups).

### Store open

| Metric | Budget | Status | Notes / provenance |
| :-- | :-- | :-- | :-- |
| CLI store-open wall | -- | **observational** | `benchmarks/store-open.md` -- no product wall gate. |
| Durable store open wall ratio (helper pair) | wall ratio ≤ 3.0 | **gated** (harness) | `store_open_benchmark` in `perf_harness` (`RATIO_LIMIT`). |
| Incremental RSS after open measure | ≤ 64 MiB | **gated** (harness) | Same scenario (`RSS_LIMIT_BYTES`). |

### World fork

| Metric | Budget | Status | Notes / provenance |
| :-- | :-- | :-- | :-- |
| Median per-fork wall | < 10 ms | **gated** (runner table) | `benchmarks/world-fork.md` / `world_fork.py` -- historical PASS at 23k and 100k files on M5 Max. |

### Watch / save freshness

| Metric | Budget | Status | Notes / provenance |
| :-- | :-- | :-- | :-- |
| save → queryable p50 / p95 | < 1 s | **gated** (tests) | `save_to_queryable_freshness_under_1s_p50` in `tests/watch_mode.rs` ([`index-trust.md`](index-trust.md)). |
| per-save index apply p50 | < 1 ms (release) | **gated** (tests) | `per_save_index_cost_under_1ms_p50`. |
| Watch bake-off competitor walls | -- | **observational** | `benchmarks/watch-bakeoff.md` -- fair comparison fingerprints, not product SLOs. |

### Search / edit (small repo)

| Metric | Budget | Status | Notes / provenance |
| :-- | :-- | :-- | :-- |
| Search p95 (≈60-file repo) | ≤ 100 ms | **aspirational** | DEFINE 20260704_001; capture included index build (~0.57 s) -- re-measure warm-index-only before treating as green. |
| Edit p95 (≈60-file repo) | ≤ 100 ms | **aspirational** | Same DEFINE; capture ~0.20 s including index path. |

### Concurrent spawn / lock

| Metric | Budget | Status | Notes / provenance |
| :-- | :-- | :-- | :-- |
| Integrity (locked/warm children) | zero failed | **gated** | `benchmarks/concurrent-spawn.md` DEFINE success metrics. |
| Unlocked:locked aggregate CPU ratio | -- | **observational** | Historical ~53× at N=8 is a fingerprint, not a CI floor. |

## Emergency rollback

If a gated budget regresses on the same host class:

1. Stop re-baselining (variance envelope rules).
2. File a bead with raw trial vector + fingerprint + offending commit.
3. Prefer revert of the suspected change over raising the budget without review.
4. Aspirational over-budget rows stay labeled aspirational -- never flip to gated
   green without fresh measurements under the target.

## Related tooling (may lag this document)

| Bead / tool | Role |
| :-- | :-- |
| `fszero-tep8` / `.1-.3` | CI surface ratchets and history schema |
| `fszero-j0c6` | Suite variance_envelope / bench_baseline scripts |
| `fszero-seju` | Promotion path: `docs/evidence/perf/<run-id>/` (scratch stays gitignored) |
| `scripts/claims_audit.py` | README claim annotations vs artifacts |
| `scripts/benchmark.sh` | Regenerates `benchmarks/demo-bench_results.json` under release-perf |

## Changelog discipline

Update this file when northstar budgets change. Prefer a short table-row edit
plus commit message; do not bury new product gates only in gitignored DEFINE
cards.
