# `perf_harness` scenario catalog

`perf_harness` contains in-process profiling scenarios. How to attach samply /
flamegraph / fingerprint artifacts: [`docs/profiling.md`](../../docs/fszero/profiling.md).

Run one scenario with:

```bash
./scripts/profile_build.sh --cargo-command bench --bench perf_harness -- <scenario> [args...]
```

`FSZERO_PERF_ROOT` selects the corpus root and defaults to the current directory.
`FSZERO_BIN` selects the surface binary and defaults to
`target/release-perf/fszero`. Except for `store_open_benchmark`, each invocation
emits one observation. Repeat it in an external runner before making a
statistical claim. "Measurement-only / no gate" means the scenario must still
complete successfully and produce its expected functional result.

## Scenario cards

| Scenario | Inputs and corpus | Expected functional outcome | Reported metric | Budget |
| :-- | :-- | :-- | :-- | :-- |
| `index_build` | `FSZERO_PERF_ROOT`; a fresh temporary store; gitignore disabled | A repository-backed session and index open successfully. | `index_build_ms` wall time | Measurement-only / no gate |
| `session_init [root]` | Optional root argument, default `.`; a fresh temporary store; gitignore disabled | A repository-backed session opens successfully. | `session_init_ms` wall time | Measurement-only / no gate |
| `read_hot [path]` | Optional workspace-relative path, default `src/lib.rs`; one unmeasured warmup read | The measured read prints `ok=true`. | `read_hot_ms` wall time | Measurement-only / no gate |
| `read_cold [path]` | Optional workspace-relative path, default `src/lib.rs`; new in-memory session | The first read prints `ok=true`. | `read_cold_ms` wall time | Measurement-only / no gate |
| `read_range [path]` | Optional workspace-relative path, default `src/lib.rs`; byte range `#B0-4096` | The range read prints `ok=true`. | `read_range_ms` wall time | Measurement-only / no gate |
| `search_struct` | Corpus root; fixed structural query `defs:main`; one unmeasured warmup query | The measured search prints `ok=true` for the fixed query. | `search_ms` wall time | Measurement-only / no gate |
| `search_grep` | Corpus root; fixed text query `fn main`; one unmeasured warmup query | The measured search prints `ok=true` for the fixed query. | `search_ms` wall time | Measurement-only / no gate |
| `resolve_empty` | Corpus root; fresh temporary indexed store; fixed absent symbol `nonexistent_symbol_xyz_zz9` | Resolution completes without a domain failure and prints its `ok` flag. No hit is expected. | `resolve_empty_ms` wall time | Measurement-only / no gate |
| `resolve_populated` | Corpus root; session primed by reads of `src/lib.rs` and `src/core/session.rs`; symbol `FSZeroSession` | Resolution completes and prints `ok=true`. | `resolve_populated_ms` wall time | Measurement-only / no gate |
| `readmany_50` | First 50 Rust files found under the corpus, excluding `target`, `.git`, `node_modules`, and `.zerostack`; one `fs.readMany` plan | The plan completes and prints its acknowledgement. | `readmany_50_ms` wall time | Measurement-only / no gate |
| `read_50_singles` | The same first-50 Rust-file selection; 50 separate `R` operations in one session | Every collected path is reported in `ok=<passed>/<collected>`. | `read_50_singles_ms` wall time | Measurement-only / no gate |
| `verified_edit_ok` | Recreated scratch file `tests/artifacts/perf/_scratch_edit.txt`; `beta` to `BETA`; verifier `true` | The verified compound edit succeeds and prints its acknowledgement. The scratch file is intentionally mutated. | `verified_edit_ms` wall time | Measurement-only / no gate |
| `verified_edit_fail` | The same recreated scratch file and edit; verifier `false` | Verification rejects the compound edit and prints a failure acknowledgement. | `verified_edit_ms` wall time | Measurement-only / no gate |
| `durable_read` | Corpus root; fresh repository store; gitignore disabled; fixed path `src/lib.rs` | Repository-backed read prints `ok=true`. | `durable_read_ms` wall time | Measurement-only / no gate |
| `memory_read` | Corpus root; new in-memory session; fixed path `src/lib.rs` | In-memory read prints `ok=true`. | `memory_read_ms` wall time | Measurement-only / no gate |
| `codemode_trivial` | Corpus root; fixed CodeMode input `explore` | CodeMode completes and prints its acknowledgement. | `codemode_trivial_ms` wall time | Measurement-only / no gate |
| `codemode_50step` | Corpus root; one plan containing 50 `fs.ls` steps with depths 1 through 3 | The full plan completes and prints its acknowledgement. | `codemode_50step_ms` wall time | Measurement-only / no gate |
| `mcp_cold_codemode` | `FSZERO_BIN`; corpus root; new CodeMode process; initialize plus one `fz_codemode_search` call | The child exits successfully after emitting the initialize and tool responses. | `mcp_cold_codemode_ms` process wall time and exit code | Measurement-only / no gate |
| `mcp_loop_50 [mode]` | `FSZERO_BIN`; corpus root; new selected-surface process; initialize plus 50 fixed tool calls | The child exits successfully and emits the expected initialize/tool response stream. | `mcp_loop_50_ms` process wall time and response-line count | Measurement-only / no gate |
| `store_open_seed <db> <rows>` | Database path and row count; rows inserted in batches of 10,000 | The durable store validates after seeding and reports validation scan counts. | Row count and validation pack/memory rows scanned | Measurement-only helper / no performance gate |
| `store_open_measure <db> <rows>` | Existing database produced by `store_open_seed`; one fresh process | The validated store reopens and emits one JSON measurement with maintenance scan counts. | Internal open wall time, process baseline RSS, and scanned-row counts | Measurement-only helper / no performance gate |
| `store_open_benchmark <output-dir>` | Fresh durable stores with 100,000 and 1,000,000 rows; 20 fresh-process reopen samples per size | Both stores seed and reopen; every sample scans zero payload rows; CPU profiling is available; the final JSON has `passed=true`. | Ordered raw wall/process/CPU/RSS/scan vectors; p50/p95/p99 wall, process, CPU, and RSS; max RSS; wall and CPU size ratios | `RATIO_LIMIT = 3.0` for wall and CPU; `RSS_LIMIT_BYTES = 64 MiB`; zero payload rows scanned; unavailable CPU profiling fails |

The `store_open_benchmark` limits above are defined by the constants with the
same names in `benchmarks/perf_harness.rs`. Change the code and this card in the
same reviewed re-baseline. The two `store_open_*` helper scenarios do not form
independent performance gates.

`benchmarks/store_open.py` stamps both `host_class` and the full scenario
fingerprint. Run the gate on its Linux RCH/Spark class with explicit identity
and isolation:

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero_profile \
  FSZERO_PERF_RUNNER_CLASS=linux-rch-spark \
  FSZERO_PERF_ISOLATION_NOTE="isolated Spark worker; no competing load" \
  make -f benchmarks/durable_store_open.mk durable-store-open
```

A native Darwin observation must declare a different class, such as
`darwin-local`. Never compare its absolute wall, CPU, or RSS values to the
Linux RCH/Spark gate. Recalibrate a separate host-class threshold instead.
