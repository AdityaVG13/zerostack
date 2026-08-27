# Hot RecoveryStore SQL EXPLAIN catalog (fszero-sql-explain-hot-catalog-botu)

## Gate

`FSZERO_SQL_EXPLAIN=1` enables automatic dumps via
`RecoveryStore::maybe_write_sql_explain_artifacts`. Harnesses may always call
`capture_sql_explains()` / `write_sql_explain_artifacts` without the env.

## Outputs

`tests/artifacts/perf/<run-id>/db/explain_<name>.txt` plus `explain_summary.json`.

Each file includes SQL text, `PreparedStatement::explain()` VDBE disassembly when
available, and `EXPLAIN QUERY PLAN` rows. `plan_class` is a coarse tag
(scan/index/union/temp) for human triage -- **not** a proof of index use.

## Catalog (first wave)

See `src/core/recovery/sql_explain.rs::hot_sql_catalog`.

This bead **does not** add indexes; product follow-ups use the artifacts as
evidence.
