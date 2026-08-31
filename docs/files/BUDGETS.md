# Files performance budgets

Only ZeroKernel-owned execution is a current product gate. Historical results from
per-domain runners do not establish a current wall-time, RSS, or binary-size threshold.

The Rust implementation still enforces semantic bounds such as output limits, snapshot
budgets, and deterministic truncation. Those are correctness contracts, not measured
latency claims.

A new public performance budget must include a live runner against the standalone
`zero-kernel` CLI or Rust API, the exact command and source revision, a clean
fixture, host and toolchain fingerprints, at least 20 samples for percentile claims,
raw sample vectors, and fail-closed threshold evaluation.

See [Benchmark integrity](benchmark-integrity.md) for the measurement rules.
