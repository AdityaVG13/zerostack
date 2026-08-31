# Tokens benchmarks

TokenZero benchmarks exercise the internal domain library and make no product claim without current, same-unit evidence.

Current benchmarks exercise typed crates directly:

- `tokenzero-core`: `hotpaths`

Run one named target through RCH. Example:

```bash
RCH_REQUIRE_REMOTE=1 CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  rch exec -- cargo bench -p tokenzero-core --bench hotpaths --profile release-perf
```

The never-worse product promise has no live clean-cutover benchmark driver. Treat it
as unproven until a same-unit benchmark exercises direct ZeroKernel execution and
publishes exact inputs, outputs, tokenizer identity, host class, and sample vectors.
