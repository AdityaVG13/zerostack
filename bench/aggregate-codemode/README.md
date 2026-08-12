# Aggregate CodeMode benchmark

This provider-free harness compares the supported in-process `zsx-core` runtime at three orchestration granularities:

- one aggregate model-visible cell;
- one standalone CodeMode cell per participating engine;
- one model-visible cell per logical operation.

The retired sidecar is not revived or timed. Its supported successor is identified explicitly in every receipt. The corpus covers mixed three-engine work, exact refs, shell, mutation, cancellation, and failure. Receipts retain every raw cold/warm trial, CPU/RSS samples, bytes, conservative token accounting, model-visible turn counts, percentiles, variance, losses/ties, differential correctness, source state, machine identity, and addon digest.

Run a native measurement without invoking Cargo:

```sh
node bench/aggregate-codemode/bench.mjs \
  --cold-runs 2 --warm-runs 10 --repetitions 2 --warmup 1 \
  --output bench/aggregate-codemode/results/native-macos-arm64.json
node bench/aggregate-codemode/validate.mjs \
  bench/aggregate-codemode/results/native-macos-arm64.json
```

The committed addon is bound by SHA-256. The receipt deliberately says `digest_only` because the binary does not embed a verifiable source revision. Exact provider token counts are also `null`; the benchmark makes no model call and reports a labeled conservative UTF-8/JSON estimate instead.
