# zero-cert resident verification benchmark

Status: **measured on final candidate code through Spark RCH**.

Command:

~~~sh
RCH_VISIBILITY=verbose RCH_PRIORITY=high RCH_FORCE_REMOTE=true rch exec -- cargo bench -p zero-cert --bench verify -- --noplot
~~~

Result:

- Environment: `spark-1672`, isolated clean worktree, 2026-07-28
- Benchmark: `zero_cert_verify/resident_64k`
- Resident object size: 65,536 bytes
- Time: 344,150 ns/iteration (point estimate)
- Criterion 95% confidence interval: 344,100-344,190 ns/iteration
- Throughput: 181.61 MiB/s point estimate (181.58-181.63 MiB/s interval)
- Allocation gate: zero allocations on the successful resident-object verification path, enforced by `success_path_allocates_nothing`

The RCH completion marker was `[RCH] remote spark-1672`; local fallback was disabled and not accepted.
