# GraphZero benchmarks

Current public latency claims come from committed, reproducible artifacts. Each artifact records binary digest, corpus, profile, host class, sample count, percentile estimator, and dropped-sample accounting.

## Current rebaseline

Source: [`benchmarks/rebaseline/latest.json`](../benchmarks/rebaseline/latest.json)

| Operation | p50 | p95 | Runs |
| --- | ---: | ---: | ---: |
| Warm orient on a symbol | 24.172 ms | 24.532 ms | 20 |
| Blast radius | 19.587 ms | 24.918 ms | 20 |
| Warm re-index | 57.959 ms | 63.160 ms | 20 |

Reference environment: release-perf binary on a dedicated macOS arm64 RCH runner, 365 Rust files, no samples dropped.

Cold-index and verify latency remain unpublished until normalized fixtures meet the sample floor. Historical CodeMode and standalone-MCP artifacts do not describe the current ZeroKernel adapter and must not be compared with this table.

## Reproduce

```bash
./scripts/benchmark.sh
```

A new number replaces a claim only when the method and corpus remain comparable or the semantic reason for the change is documented.
