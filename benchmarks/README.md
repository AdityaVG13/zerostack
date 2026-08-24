# ZeroStack benchmarks

Public benchmark claims must include a reproducible harness, retained samples, environment metadata, and explicit scope.

| File | Purpose |
| --- | --- |
| [`zero-kernel-reference.cjs`](zero-kernel-reference.cjs) | Reusable-host benchmark harness |
| [`zero-kernel-reference.json`](zero-kernel-reference.json) | Current samples, percentiles, fixture, and environment |

Run from the repository root:

```bash
node benchmarks/zero-kernel-reference.cjs
```

The reference measures host initialization, fresh no-op frames, and a bounded file read. It does not claim concurrency throughput, memory use, or cross-machine equivalence.
