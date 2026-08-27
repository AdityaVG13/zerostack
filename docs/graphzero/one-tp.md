# GraphZero 1TP adoption

GraphZero uses the coordinated `zero-gauge` crate at revision
`fa253840910ab4051635e2de95f04ddf6043a000` for canonical ordinal syntax:
`gz://o/<snapshot-generation>/<one-based-ordinal>`. GraphZero's graph nodes
are its global symbols. Symbol ordinals are dense and precede semantically
ordered CSR edge ordinals in the durable snapshot sidecar.

A successful domain operation with explicit `budget: 0` returns
`graphzero.one_tp_ack.v1`. The ack contains `ack: "C"`, snapshot generation,
ordinal counts, and registered operation TA metadata, but no original payload.
Missing, corrupt, or snapshot-mismatched ordinal substrate fails closed as a
`substrate` error. Nonzero and omitted budgets retain existing behavior.

TA entries are registered bounds only. TokenZero owns token measurement and
provider certification. GraphZero does **not** claim one-token behavior or
provider-locked token certification.

Retained orient baseline: `orient.orient_symbol_p50_ms = 9.358 ms` from
`benchmarks/latency/results.json`. End-to-end comparisons must name the active profile and
are advisory when the test/debug profile differs.

Focused verification:

```text
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo test -p graphzero-query --test one_tp_protocol -- --test-threads=1
```
