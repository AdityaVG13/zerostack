# ZeroStack benchmarks

Public benchmark claims must include a reproducible harness, retained samples, environment metadata, and explicit scope.

## Product reference

| File | Purpose |
| --- | --- |
| [`zero-kernel-reference.cjs`](zero-kernel-reference.cjs) | Reusable-host benchmark harness |
| [`zero-kernel-reference.json`](zero-kernel-reference.json) | Current samples, percentiles, fixture, and environment |

Run from the repository root:

```bash
node benchmarks/zero-kernel-reference.cjs
```

The reference measures host initialization, fresh no-op frames, and a bounded file read. It does not claim concurrency throughput, memory use, or cross-machine equivalence.

## Domain evidence

Capability folders hold domain-specific harnesses and claim artifacts. They are not separate products.

| Path | Purpose |
| --- | --- |
| [`files/`](files/) | Files-domain claim notes and feed contracts |
| [`structure/`](structure/) | Structure-domain crash-boundary corpus and latency gates |
| [`tokens/`](tokens/) | Tokens-domain keep-gate validator and its tests |
