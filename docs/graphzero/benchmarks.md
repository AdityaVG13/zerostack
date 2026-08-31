# GraphZero benchmarks

GraphZero benchmarks exercise internal domain libraries. Historical measurements from other execution surfaces are not product claims.

Current benchmarks exercise typed domain crates directly:

- `graphzero-engine`: `query_surface`
- `graphzero-store`: `budgets`, `query_warm`, `query_cold`, `branch_switch`
- `graphzero-extract`: `extraction_bench`
- `graphzero-reserve`: `reserve_check`
- `graphzero-coverage`: `coverage_bench`

Run one named target through RCH. Example:

```bash
RCH_REQUIRE_REMOTE=1 CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  rch exec -- cargo bench -p graphzero-store --bench budgets --features benchmarks
```

Publish a number only with the exact command, source revision, profile, host class,
corpus identity, sample count, percentile method, and complete dropped-sample record.
