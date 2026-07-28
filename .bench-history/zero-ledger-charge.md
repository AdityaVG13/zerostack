# zero-ledger charge benchmark

Status: **measured on final candidate code through Spark RCH**.

~~~sh
RCH_LOG_LEVEL=error RUST_LOG=error RCH_VISIBILITY=summary RCH_FORCE_REMOTE=true rch exec -- cargo bench -q -p zero-ledger --bench charge -- --noplot
~~~

- Host: `spark-1672`
- Benchmark: `zero_ledger_charge`
- Point estimate: 15.597 ns
- Criterion interval: 15.594-15.600 ns
- Allocation gate: `warmed_successful_charge_allocates_nothing` passed with zero allocations
- RCH marker: `[RCH] remote spark-1672`
- Local fallback: disabled and not accepted
