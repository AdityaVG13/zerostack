# Benchmark scenarios

`benchmarks/latency/scenario_catalog.json` is the machine-readable authority. Use one named
scenario before collecting or comparing performance evidence. A row marked
**measure-only** has no pass/fail threshold; its observed value is not an SLO.
Profiles remain distinct: ordinary `release`, `release-perf`, and
`cargo-test-dev` evidence must never be merged or relabeled.

| Name / surface | Frozen input and corpus | Expected output class | Success metric and evidence |
|---|---|---|---|
| `cold_index_tracked_rust` / cold index | `target/release/graphzero index .`; new isolated empty store; tracked corpus at the fixture `git_rev` | exit 0; published snapshot manifest | **measure-only**; `benchmarks/latency/results.json#cold_index`; policy: `benchmarks/latency/setup_surface_policy.json` |
| `warm_reindex_tracked_rust` / warm index | same command against the already indexed tracked corpus | exit 0; current snapshot remains queryable | **measure-only**; `benchmarks/latency/results.json#warm_reindex`; policy: `benchmarks/latency/setup_surface_policy.json` |
| `orient_symbol_run_index` / orient | release CLI, symbol `run_index`, budget 1; recorded corpus, at least 200 Rust files | exit 0; budgeted symbol response with expandable refs when detail spills | p50 <=120 ms, p95 <=150 ms, <=2x comparable baseline; retained fixture is baseline-incomparable and has fewer than 20 samples; `benchmarks/latency/latency_gate.json` |
| `blast_change_signature_run_index` / blast | release CLI, intent `change signature of run_index`, budget 1; same corpus rule | exit 0; budgeted blast response with expandable refs | p50 <=15 ms, p95 <=25 ms, <=2x comparable baseline; retained fixture currently fails the absolute gate; `benchmarks/latency/results.json#blast` |
| `queryengine_scaled_200` / library query | `sym_100`, budget 1, generated 200-file fixture | successful `Capsule` | warm: 128 samples, median <=10 ms, p99 <=50 ms; cold: 32 samples, median <=10 ms, p99 <=20 ms; `benchmarks/latency/query_latency_gate.json` |
| `snap_cli_run_index` / snap | release CLI, `run_index`, budget 1; recorded corpus | exit 0; compact capsule with exact expandable refs | **measure-only** because the seven-run fixture has p50 only; `benchmarks/latency/results.json#snap`; policy: `benchmarks/latency/setup_surface_policy.json` |
| `wal_segment_pressure_reopen` / store open | release example, 11 WAL segments, 256 entries each, 9 trials, tracked self corpus >=600 Rust files | every open succeeds; segment counts 11 before and 0 after | replay p50 <=180 ms / p95 <=200 ms; cached p50/p95 <=0.1 ms; `benchmarks/latency/wal_open_compaction_gate.json`; `benchmarks/latency/wal_open_compaction.json` |

## Evidence rules

- Use the exact command/API, profile, corpus, and output class in the selected row.
- Keep ordered samples and bind the binary digest, host class, and isolation for
  any promoted public claim.
- Do not infer a threshold from a measure-only observation.
- Do not call a retained failing fixture green. Rebaseline only through its
  named producer and preserve prior measured artifacts.
