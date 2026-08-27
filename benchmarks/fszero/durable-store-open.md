# Durable store-open DEFINE card

## Scenario

`durable-store-open` measures reopening the same validated durable store at
100,000 and 1,000,000 rows. It tests size independence. It does not measure a
cold integrity rebuild and it does not permit a reduced row count.

Authoritative surfaces:

- [Python driver](store_open.py) owns provenance, the 20-run floor, tails, and
  loud JSON publication.
- [Rust helper](perf_harness.rs) owns `store_open_seed`, `store_open_measure`,
  and the integrated `store_open_benchmark` constants.
- [Make target](durable_store_open.mk) owns the canonical release-perf command.
- [Committed evidence](durable-store-open.json) preserves the last measured
  outcome. It is evidence, not a target to copy.

## Inputs and setup

| input | canonical value |
| :-- | :-- |
| helper profile | `release-perf`, built and run only through RCH |
| row counts | `100000,1000000` |
| database path | session scratch `<work>/<rows>/store.sqlite3` |
| seed operation | `store_open_seed <db> <rows>` once per row count |
| measured operation | `store_open_measure <db> <rows>` in a fresh child |
| Python measured opens | 20 per row count, no excluded warmup or outlier |
| integrated Rust measured opens | 20 per row count, with the same raw-vector policy |
| cache class | warm durable reopen after the seed validation |

The seed child must create the exact row count and validate the store. Every
measured child must reopen that existing database. A successful reopen scans
zero payload rows. Pack and memory maintenance scan counts stay in the raw
sample as attribution; they never excuse a payload scan.

## Metrics and budgets

| output | success rule |
| :-- | :-- |
| internal open wall | report p50, p95, p99, and every ordered raw sample |
| fresh-process wall | report p50, p95, p99, and every ordered raw sample |
| child CPU | report p50, p95, p99, and every ordered raw sample |
| CPU profiler availability | Linux `/proc` exists and an out-of-band CPU probe records positive ticks; an unsupported OS or zero measured CPU p50 makes the ratio unavailable and fails |
| payload rows scanned | exactly 0 on every measured reopen |
| maximum incremental peak RSS | at most 64 MiB (67,108,864 bytes) at each row count |
| p50 internal-wall size ratio | at most 3.0, using max/min across the two row counts |
| p50 CPU size ratio | at most 3.0, using max/min across the two row counts |

The 3.0 ratio and 64 MiB limits bind both the Python defaults and
`RATIO_LIMIT` / `RSS_LIMIT_BYTES` in `run_store_open_benchmark`. A threshold
change requires a reviewed re-baseline. Never raise a limit to turn a failure
into a pass. If the CPU p50 is zero at the profiler tick resolution, the JSON
uses `null` for the CPU ratio and records a failure instead of treating zero
as an eligible passing ratio.

## Status and publication

The Python driver initializes `status` to `fail`, records all samples and
failures, writes the JSON, and then exits nonzero when any rule fails. It may
emit `status: pass` only when every functional and budget rule succeeds.

The committed artifact currently records `status: fail`. It reports zero
payload scans, but its measured RSS and wall/CPU ratios exceed the unchanged
budgets. It is a historical five-run artifact from an earlier producer shape.
Do not present it as passing, hand-edit it, or overwrite it during a docs or
code-only change.

The integrated Rust `store_open_benchmark` uses 20 measured opens per size,
computes p50/p95/p99 from sorted copies, and emits every ordered wall, CPU,
RSS, and scan vector. Its scratch output follows the same fail policy; it does
not replace the provenance-stamped committed artifact.

## Reproduce

Run the integrated scenario only on a Linux RCH/Spark runtime. The Make target
selects the `release-perf` profiling wrapper and writes only scratch output.
The result is eligible only when `cpu_profiler.target_os` is `linux`,
`procfs_self_stat_available` is `true`, and `available` is `true`; setting the
runner-class environment value does not override a mismatched runtime:

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero_durable_open \
  FSZERO_PERF_RUNNER_CLASS=linux-rch-spark \
  FSZERO_PERF_ISOLATION_NOTE="isolated Spark worker; no competing load" \
  make -f benchmarks/durable_store_open.mk durable-store-open
```

For a provenance-stamped JSON publication, invoke the
[Python driver](store_open.py) with the resulting release-perf
`perf_harness` executable as `--helper`. Publication still requires the
matching host class and isolation fields from
[benchmark integrity policy](../docs/benchmark-integrity.md).

## Memory dual metric (fszero-5444)

Each open sample includes `peak_rss_bytes` (GNU time) and `heap_high_water_bytes` (null + `unsupported` reason until an allocator profiler is wired). See `docs/profiling.md#dual-metric-peak-rss-vs-heap-high-water`. Do not treat empty MALLOC_LARGE as live heap.
