# Re-baseline methodology

`benchmarks/rebaseline/run.py` refreshes current GraphZero Northstar numbers and keeps append-only history in `history.jsonl`.

## Python environment contract

All benchmark/rebaseline/ratchet Python drivers are **stdlib-only** plus the local module `benchmarks/rebaseline/stats.py`; there are no third-party imports (verified by `scripts/test_python_benchmark_environment.py`, which walks the import closure of the four entry modules). The environment contract lives at the repository root: `.python-version` (3.13) selects/pins the interpreter minor, a non-package `pyproject.toml` (`requires-python = ">=3.13,<3.14"`, zero dependencies) declares the compatibility bound, and `uv.lock` locks dependency resolution (here: none; it does not record an interpreter). Run any driver through `uv run --locked --no-dev python ...` so the interpreter matches `.python-version` and the environment never falls back to an ambient `python3`.

Clean locked smoke (no Cargo or benchmark execution; uv may install the pinned interpreter on first use):

```bash
uv run --locked --no-dev python -m unittest scripts.test_python_benchmark_environment scripts.test_bench_ratchet scripts.test_benchmark_artifact -v
cd benchmarks/rebaseline && uv run --locked --no-dev python -m unittest test_run -v
```

(`test_run` imports its sibling `stats` through the module directory, so it is
run from `benchmarks/rebaseline/` where that sibling resolves; the other
suites run from the repository root.)

Select the frozen input/output/metric contract from
[`benchmarks/latency/scenario_catalog.json`](../latency/scenario_catalog.json)
before a run. This producer owns the named warm-reindex, orient, and blast
scenarios; it must not invent an ad-hoc replacement.

## Build profile and binary

The runner currently defaults to Cargo's ordinary `release` profile. It builds with `cargo build --release -p graphzero-cli` when needed and measures `target/release/graphzero`. That path is distinct from the host-timed release-gate profile `release-perf`, whose binaries live under `target/release-perf/`.

`[profile.release-perf]` in the workspace `Cargo.toml` is the canonical profile definition. Host-timed release gates must name `--profile release-perf`. To produce a rebaseline under that profile, set `GRAPHZERO_BENCH_PROFILE=release-perf`, `GRAPHZERO_BENCH_HOST_CLASS`, and `GRAPHZERO_BENCH_ISOLATION` for one RCH build-and-run. If `GRAPHZERO_BIN` overrides the binary, it must name the exact binary from that same remote run. The receipt records the profile, portable target-relative binary path, binary digest, host class, and isolation; missing host or isolation fails closed. Never compare, merge, or re-label `release` and `release-perf` samples.

It measures, on the current GraphZero working tree:

- warm reindex latency after a priming index;
- `orient --surface symbol --name run_index` p50/p95/p99;
- `blast --intent "change signature of run_index"` p50/p95/p99;
- per-save incremental update status, currently recorded as `not_available` until the API lands.

Latency metrics discard **W warmups** then retain **N measured samples**. Defaults: W=1 (`GRAPHZERO_BASELINE_WARMUP`), N=20 (`GRAPHZERO_BASELINE_RUNS`). Warmups never enter p50/p95/p99 or `samples_ms`. They are intentional protocol discards after a separate store-priming index, not sample losses: `sample_accounting.dropped_count` stays 0 when no measured sample is lost. Both W and N are recorded in `sample_accounting` (`warmup_discarded`, `measured_retained`, plus totals and per-metric breakdown).

p50 uses the sample median. p95 and p99 use Hyndman-Fan type 7 interpolation, matching NumPy/R's default quantile method, so small sample sets do not silently report max-of-N as the percentile. Each latency metric publishes `p99_ms` plus `p99_label`: when N < 200 the label is `worst_observed_of_n` (type-7 p99 is near the sample max and is not a population-p99 claim); at N >= 200 the label is `hyndman_fan_type7`.

## Variance envelope (CV / drift ≤ 10%)

Before a baseline is trusted for publication, the runner evaluates a **variance envelope** on the retained sample series (warmups already discarded):

| Signal | Definition | Envelope |
|---|---|---|
| CV | sample stdev / mean over `samples_ms` | ≤ 10% publishable; ≤ 5% stable; > 10% investigate; > 20% escalate |
| MAD/median | median absolute deviation / median | companion robust scale (reported, not the primary gate) |
| Same-host drift | \|current p50 − prior p50\| / prior p50 for matching `host_class` + `isolation` + `profile` in `history.jsonl` | ≤ 10% noise; > 10% reject |

Each latency metric stamps `cv`, `cv_pct`, `mad_ms`, `mad_over_median`, and `variance_envelope` while keeping full `samples_ms` for audit. The report root carries:

- `variance_envelope` -- aggregate CV gate (`status`: `pass` / `reject`, `failed_metrics`)
- `same_host_drift` -- prior-run drift gate (`status`: `pass` / `reject` / `no_prior`)

Default mode is **strict** (`GRAPHZERO_BASELINE_VARIANCE_MODE=strict`): the process exits non-zero when CV or same-host drift exceeds 10%, after still writing `latest.json` / history for diagnosis. Set `GRAPHZERO_BASELINE_VARIANCE_MODE=advisory` to stamp and warn without failing.

`scripts/benchmark_driver.py` applies the same per-metric CV fields on every series with `raw_ms`. Its default mode is **advisory** (`GRAPHZERO_BENCH_VARIANCE_MODE`); set `strict` to fail closed there too.

The script emits both machine JSON (`latest.json`) and Northstar-ready Markdown (`latest.md`).



## Process-start vs op split (CLI wall class)

Published `orient_symbol` / `blast` / `warm_reindex` p50/p95/p99 values are
**process-inclusive wire time**: `fork/exec + dyld + main + Snapshot/dispatch +
serde + exit`. They are not in-process library timings.

Each rebaseline also measures a separate `cli_process_start` series by timing
`graphzero --version` (minimal CLI: no store open, no query). That series is
stamped on every process-inclusive metric as `process_start_ms`, and an
estimated residual is published as `op_ms` via
`wall_quantile − process_start_quantile` (not a paired per-sample split; the
public CLI has no child-reported wall on this path).

Field contract:

| Field | Meaning |
|---|---|
| `wall_class: process_inclusive` | Existing p50/p95/p99 include process envelope |
| `process_start_ms` | Probe quantiles from `graphzero --version` |
| `op_ms` | Estimated residual after process-start probe |
| `cli_process_start` (root metric) | Full process-start sample series |

Dual harness: `crates/graphzero-query` `surface_bench` records `process_starts`
and `spawn_ns` (parent wall − child wall) for o2uq gates. Those numbers are
**not** what `benchmarks/latency/results.json` / rebaseline publish; the Python drivers own
the public CLI split fields above.

## Measure-only setup surfaces

The broader `scripts/benchmark_driver.py` also records CLI snap and full cold
index observations. `benchmarks/latency/setup_surface_policy.json` is authoritative: neither
observation is a latency SLO or pass/fail gate. The driver publishes
p50/p95/p99 for snap and other public latency metrics (with the same small-N
p99 label). Cold index has no normalized corpus throughput. Do not promote
setup-surface numbers into a Northstar claim until the policy's sample,
provenance, metric, and producer-gate requirements are met.


## Host fingerprint and isolation

Comparable baselines require a full host fingerprint in `latest.json#hardware`. The collector always emits every key; probes that are unavailable on the host are `null` (cpu falls back to `"unknown"`). Keys:

- `cpu` -- model/brand string
- `os` / `kernel` / `platform` / `machine` -- OS identity
- `memory` -- total RAM label, or `null`
- `python` / `rustc` -- interpreter and toolchain versions (`rustc` is `null` if missing)
- `profile` -- Cargo profile under measurement (`release` or `release-perf`)
- `fs_type` / `store_path` -- filesystem type for the graph store root (portable label `.zerostack/graphzero`); `fs_type` is `null` when unprobeable
- `governor` / `power_mode` -- Linux cpufreq governor / energy preference when sysfs exposes them; macOS may record `pmset` low-power mode; otherwise `null` (not omitted)
- `load_average` -- optional 1/5/15-minute load averages, or `null`
- `host_class` / `isolation` -- mirrored from `GRAPHZERO_BENCH_HOST_CLASS` and `GRAPHZERO_BENCH_ISOLATION` (required for a rebaseline run; also under `measurement_environment`)

### Same-host discipline

Rebaseline and `scripts/benchmark_driver.py` measurements meant for publication must run on a **dedicated same-host** session:

- No concurrent heavy jobs (other full builds, parallel benchmark suites, or bulk indexing) on the same machine while samples are collected.
- Prefer a single-purpose process (RCH targeted run, `taskset` / cpuset when available) and record that discipline in `GRAPHZERO_BENCH_ISOLATION` (e.g. `dedicated RCH process; no concurrent GraphZero benchmark; taskset 0-7`).
- Do not merge samples across hosts, host classes, governors, filesystems, or isolation modes. Missing fingerprint fields stay `null` so consumers can fail closed rather than inventing sameness.

`scripts/benchmark_driver.py` records the same hardware key set. `host_class` / `isolation` are optional there (null when unset) but required for any result compared as a Northstar or release-gate baseline.

## Freshness manifest

latest.json is a live measurement report. Its freshness block records the SHA-256 of run.py, stats.py, this methodology file, and a deterministic digest of every Rust corpus file used by the benchmark. The report gate recomputes those digests so source, methodology, or generator changes fail until the report is regenerated.


## MCP / CodeMode handshake boundary

`scripts/benchmark_driver.py` measures MCP warm orient and CodeMode over long-lived
stdio servers. Steady-state `mcp_orient_*` / `codemode_*` tools/call samples remain
post-handshake. The driver also publishes:

- `server_spawn_ms` -- wall from before `Popen` to after process create
- `initialize_ms` -- JSON-RPC `initialize` round-trip
- `time_to_first_tool_ms` -- wall from session start to first successful `tools/call`
- `process_cold_start_included: false` -- tools/call p50 is not process cold-start

Do not label `cold_orient` as process cold-start without including spawn+initialize.


## Foreign corpora (non-self integrity)

Self-repo-only latency numbers hide scale cliffs. Foreign corpora are registered in
[`benchmarks/foreign_corpora/pins.json`](../foreign_corpora/pins.json).

| Corpus id | Role | Materialization |
|---|---|---|
| `rust-mini` / `ts-mini` | In-tree non-self fixtures for harness proof | already under `fixtures/` |
| `rust-regex` @ tag 1.11.1 | Large Rust scale pin | `benchmarks/foreign_corpora/materialize.sh` |
| `ts-typescript` @ tag v5.7.3 | Large TS monorepo pin | same script |

### Running a foreign corpus rebaseline

```bash
export GRAPHZERO_BENCH_HOST_CLASS=local-dev
export GRAPHZERO_BENCH_ISOLATION=manual-no-concurrent-load
export GRAPHZERO_BASELINE_CORPUS=benchmarks/foreign_corpora/fixtures/rust-mini
export GRAPHZERO_BASELINE_CORPUS_ID=rust-mini
export GRAPHZERO_BASELINE_SYMBOL=run_index   # must exist in that corpus
# For large pins first:
# ./benchmarks/foreign_corpora/materialize.sh
# export GRAPHZERO_BASELINE_CORPUS=benchmarks/foreign_corpora/materialized/rust-regex
uv run --locked --no-dev python benchmarks/rebaseline/run.py
```

Reports stamp `corpus.is_self_repo=false`, `corpus.corpus_id`, and a corpus content
digest distinct from the self-repo digest. Headline claims must publish self **and**
foreign numbers side-by-side; self-only is not integrity-complete for scale claims.

Threshold targets (misses become beads, never silent drops):

- orient p50 < 10 ms (feeds hardware-variance gate)
- blast p50 < 25 ms
- cold index < 5 s on large foreign corpus

## Host fingerprint completeness

Hardware blocks always emit cpu, os, kernel, machine, memory, python, rust/rustc, profile,
fs_type, store_path, governor, power_mode, load_average, host_class, and isolation. Missing
probes are JSON null (cpu falls back to "unknown"), never omitted.
