# RecoveryStore SQL statement PROFILE (fszero-sql-stmt-profile-trace-x38n)

## Gate

`FSZERO_SQL_PROFILE=1` (also true/yes/on). **Off by default** -- callback cost.

## Behavior

On RecoveryStore open (`:memory:` and durable), when gated, FSZero registers:

```text
conn.trace(TraceMask::PROFILE, Some(callback))
```

Each `TraceEvent::Profile { sql, elapsed_ns }` updates a process-global map keyed by
whitespace-normalized SQL (truncated to 160 chars). Snapshots:

- `sql_profile_top(n)` -- rows with `calls`, `total_ns`, `mean_ns`, sorted by total_ns
- `sql_profile_json()` -- structured top-20 for telemetry / perf artifacts
- `session.root_report()["sql_profile"]`

## Not this bead

- Lock/busy wait (`fiey` / `xwnf`)
- WAL checkpoint spans (`61oe`)
- Payload or prepared-cache counters (`fudk`)
