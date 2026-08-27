# CLI store-open scenario

## Definition

| Field | Contract |
| :-- | :-- |
| Name | `cli_store_open_tiny_vs_warm_repo` |
| Runner | `benchmarks/cli_store_open.py` |
| Binary | `FSZERO_BIN`, default `target/release-perf/fszero` |
| Tiny input | A generated root containing one `a.txt`; its local store starts absent. |
| Repository input | The FSZero checkout and its existing local `.zerostack/fszero/store.sqlite3` or legacy `.fszero/store.sqlite3`. |
| Operation | One unmeasured prime, then `--runs` fresh `fszero codemode 'return{ok:true}'` processes for each root. Ambient shared-store pins are removed. |
| Expected outcome | Every CLI process exits successfully and the JSON artifact contains the complete trial vectors for both roots. |
| Primary metric | `repo_minus_tiny_p50_ms`, the repository-root p50 minus the tiny-root p50. |
| Supporting metrics | Per-root p50, p95, minimum, maximum, raw trials, and the selected local store size. |
| Success budget | Observational only / no pass gate. A nonzero CLI exit fails the runner. The delta includes all root/layout/content differences and is not an isolated causal estimate. |

Run from a clean checkout with one fixed release-perf binary:

```bash
./scripts/profile_build.sh -p fs-zero --bin fszero
FSZERO_BIN=target/release-perf/fszero \
  python3 benchmarks/cli_store_open.py --runs 15 \
  --output benchmarks/store-open.json
```

`benchmarks/store-open.json` is measured historical evidence, not generated
example data. Its recorded run at commit
`8822dfab806fdbb2ab2624ad63f25899917607d1` observed tiny-root p50 8.15 ms,
repository-root p50 13.54 ms, and a 5.39 ms delta. Re-running the command
replaces those values with current measurements and provenance.

This scenario is distinct from `benchmarks/store_open.py`, which drives the
100,000-row versus 1,000,000-row durable validated-store reopen gate through
the Rust `perf_harness` helper.
