# zero-gate decide benchmark history

Status: **measured on final candidate code through Spark RCH**.

## Command

~~~sh
RCH_VISIBILITY=verbose RCH_PRIORITY=high RCH_FORCE_REMOTE=true rch exec -- cargo bench -p zero-gate --bench decide -- --noplot
~~~

| Field | Value |
| --- | --- |
| Host | `spark-1672` |
| Benchmark | `zero_gate_decide_expand` |
| Candidate base | `ba11c543238180f38dc2e14efdeccb5dc7231b16` |
| Point estimate | 13.653 ns |
| Criterion interval | 13.642-13.664 ns |
| Command exit status | 0 |
| Allocation gate | zero allocations in `successful_decide_allocates_nothing` |

The RCH completion marker was `[RCH] remote spark-1672`; local fallback was disabled and not accepted.
