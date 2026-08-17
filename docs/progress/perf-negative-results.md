# Performance Negative Results Ledger

> This ledger records performance ideas that were measured and rejected. Check it before starting a new optimization pass, and add an entry whenever a candidate is abandoned, reverted, or kept out of the tree because the benchmark matrix did not move in the intended direction.
>
> Mined verbatim from FrankenSQLite methodology (CC.md lines 479–482).

ZeroStack-specific failure terms in this pillar: `savingsbytes-as-tokens`,
`cargo-target-dir-suffix`, `estimate-labeled-exact`, `daemon-install`,
`raw-worker-planner-creep`.

## Mandatory Fields per Entry

| Field | Required? | Description |
|---|---|---|
| `date` | yes | ISO 8601 (`YYYY-MM-DD`). |
| `hypothesis` | yes | What the agent thought would help. |
| `result` | yes | `REJECTED` / `DEFERRED` / `CLOSED`. |
| `evidence` | yes | Paths, SHAs, commands, numbers. |
| `retry_condition_predicate` | yes | **LOAD-BEARING.** One of the eight forms. |

## Retry-Condition Predicate Vocabulary

Use ONE of the eight forms. Anything else fails `scripts/check_ledger_retry.py`:

1. `"Retry only if a profiler attributes a clearly-above-noise share to <COUNTER> on <WORKLOAD_SHAPE>."`
2. `"Reconsider only inside the broader <X> redesign (track as <beads_id>)."`
3. `"Worth reconsidering when <GATE> crosses <THRESHOLD>."`
4. `"Not worth retrying as a standalone patch."`
5. `"Do not retry from a cold read; use comprehensive-bench attribution instead."`
6. `"Retry condition not applicable -- the gain is structural, not numerical."`
7. `"Retry only if this workload class exhibits measurable <PROPERTY> below <THRESHOLD>."`
8. `"Blocked until <ARCHITECTURAL_DEPENDENCY> lands; track as <beads_id>."`

### Forbidden phrases (listed so the linter can skip this section)

The following phrases are invalid inside `retry_condition_predicate` and
entry bodies outside this vocabulary block:

- later
- in the future
- down the road
- if it seems important
- we should revisit
- tracked elsewhere
- TBD
- TODO
- FIXME
- maybe
- eventually
- when we have time
- if circumstances change
- future work
- might be worth trying
- someone should look at this
- interesting direction
- worth exploring

---

## Cass mine (Phase 8, 2026-08-17)

`command -v cass` -- **MISSING**. jsm lists `cass` as installable, not installed.
Cross-machine mine (local / css / csd / ts1 / ts2) was not attempted as a
silent skip; it is the blocker entry below.

Fallback (not a full cass mine):

| Source | Window | Result |
|---|---|---|
| `cass search --robot --days 60` | 60d | **BLOCKER** -- binary absent |
| `git log --since='60 days ago'` | 60d, 661 commits | hits: `rejected` 2, `abandoned` 1, `commit_race` 6, `mcp-late-ok` 1, `fszero-fail-closed` 1, `savingsBytes` 1, `host path` 5, `raw worker` 5, `planner` 3, `ClampEnd` 1; **none** for exact slugs `savingsbytes-as-tokens`, `cargo-target-dir-suffix`, `estimate-labeled-exact`, `daemon-install`, `raw-worker-planner-creep` |
| `rg` of hyphenated failure slugs in the hub tree | working tree | **none** (terms live as prose / SHAs, not as those slugs) |
| `rg` over local Claude project transcripts | host session store | universal terms hit other repos; ZeroStack-specific slugs **none** |

Provenance stamp: `cass_partial_or_skipped_at_2026-08-17`. Re-run the cass
mine before declaring gauntlet convergence.

---

## Entries

### 2026-08-17 -- cass-unavailable-phase8 -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Mine 60 days of cass session history (local + css + csd + ts1 + ts2) before any perf-affecting change, as required by the AGENTS.md mandate.
- **result:** DEFERRED
- **evidence:** `command -v cass` failed on this host. `jsm list` shows `cass` (2026-07-26) as an uninstalled skill. Pass 1 recorded the same gap. Fallback: `git log --since='60 days ago'` (661 commits) plus `rg` of failure terms. Not a silent skip.
- **retry_condition_predicate:** "Blocked until cass lands on PATH (`command -v cass` succeeds and `cass health --robot` is green); track as cass-on-path."
- **files_touched:** `no-source-patch-attempted`
- **bead_id:** cass-on-path

### 2026-08-17 -- cass-unavailable-phase0 -- CLOSED

- **date:** 2026-08-17
- **hypothesis:** Pass 1 cass absence was a one-off host gap that Phase 8 would close by installing cass.
- **result:** CLOSED
- **evidence:** Re-probed in Phase 8. cass is still absent. The live blocker is `cass-unavailable-phase8`. This seed is retired so agents do not treat the Phase 0 row as the current gate.
- **retry_condition_predicate:** "Blocked until cass lands on PATH (`command -v cass` succeeds and `cass health --robot` is green); track as cass-on-path."

### 2026-08-17 -- savingsbytes-as-tokens -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Quote `result_finalization_receipt.v1.savingsBytes` as token savings on the savings-bench headline.
- **result:** REJECTED
- **evidence:** `benchmarks/savings-bench-v1.md` three-layer table; `conformance/CONTRACT.md` §3 (`savingsBytes` is not a token count); `[SPEC-RES-003]`; `benchmarks/savings-bench-v1.json` `do_not` list. Envelope bytes (e.g. 37546 on unbudgeted `token.read`) are not Exact tokens (headline billed/raw = 198/4500 = 0.044).
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."
- **failure_term:** `savingsbytes-as-tokens`

### 2026-08-17 -- cv-pct-null-savings-baseline -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Treat the committed savings-bench seed (`primary_score = 0.044`) as a keep-eligible win or as proof the port is faster than a reference.
- **result:** REJECTED
- **evidence:** `.bench-history/savings-bench.latest.json`: `cv_pct` is JSON `null`, `keep_eligible = false`, `geomean_ratio`/`p90_ratio`/`throughput` are null. Pass 5 named this: a null `cv_pct` is unknown, not a win. `scripts/check_bench_history.py` treats null or `cv_pct > 5` as noise.
- **retry_condition_predicate:** "Retry only if this workload class exhibits measurable cv_pct below 5.0 on a replicated `release-perf` savings-bench run with keep_eligible true."

### 2026-08-17 -- release-perf-lying-profile -- CLOSED

- **date:** 2026-08-17
- **hypothesis:** The in-repo `release-perf` profile (no line tables, symbols stripped) was honest enough for flamegraph / samply attribution.
- **result:** CLOSED
- **evidence:** Pass 1 named the lying profile. Pass 5 aligned `debug = "line-tables-only"`, `strip = false`, `opt-level = 3` in `Cargo.toml` and wrote a named fat-LTO waiver versus the gauntlet thin-LTO template. Frame pointers stay a `RUSTFLAGS` concern (Cargo has no per-profile key).
- **retry_condition_predicate:** "Retry condition not applicable -- the gain is structural, not numerical."

### 2026-08-17 -- fat-lto-to-thin-switch -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Flip `release-perf.lto` from `"fat"` to `"thin"` to match the gauntlet template and shrink link time.
- **result:** DEFERRED
- **evidence:** Pass 5 waiver in `Cargo.toml` + `phase5_perf_harness.md`. Fat LTO is shared with `release-node`. Switching thin would change the binary versus every prior local measurement and the shipped Node addon family. No keep was claimed for fat LTO.
- **retry_condition_predicate:** "Worth reconsidering when the release-node profile family also leaves fat LTO, or when a dedicated rch release-perf flamegraph run shows missing frames under fat LTO."

### 2026-08-17 -- cargo-target-dir-suffix -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Invent a new `CARGO_TARGET_DIR` under a bare `/tmp` (or `${TMPDIR}`) per task, without `RCH_TARGET_BASE`.
- **result:** REJECTED
- **evidence:** Target `AGENTS.md` RCH section: always use `${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}` as the base. Gauntlet heavy cargo: `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack`. Bare `/tmp` suffixes collide across agents and poison incremental caches.
- **retry_condition_predicate:** "Do not retry from a cold read; use comprehensive-bench attribution instead."
- **failure_term:** `cargo-target-dir-suffix`

### 2026-08-17 -- estimate-labeled-exact -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Label a TokenZero-missing estimate (or a backfilled `cv_pct` / throughput) as Exact so the savings-bench headline looks complete.
- **result:** REJECTED
- **evidence:** `conformance/CONTRACT.md` §5: estimates MUST NOT be labeled Exact. `[SPEC-HON-004]`. Savings v1 residual: do not backfill estimates into v1. FS adapters emit `input_token_cost: 0` (uncertified, not free).
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."
- **failure_term:** `estimate-labeled-exact`

### 2026-08-17 -- daemon-install -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Install `zsx mcp` (or an engine binary) as a machine-wide daemon / LaunchAgent so agents do not have to start a session-owned sidecar.
- **result:** REJECTED
- **evidence:** Composition law: daemonless, one session-owned sidecar, parent-death-bound. `conformance/CONTRACT.md` §2: `zsx mcp` MUST die with the parent; MUST NOT detach. `[SPEC-HUB-001]`. `F-PROCESS-IDENTITY` / serve() parent-death poll.
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."
- **failure_term:** `daemon-install`

### 2026-08-17 -- raw-worker-planner-creep -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Let the planner-free raw-worker v2 binary grow a nested CodeMode / MCP catalog / planner step so engines can "just run plans."
- **result:** REJECTED
- **evidence:** `[SPEC-HUB-004]`; `docs/codemode.md` (worker remains planner-free); `F-ABI-RAW-WORKER-V2`; `F-HUB-RAW-WORKER-DOC`. Engine topology lists `planner` as a distinct role from `raw-worker`.
- **retry_condition_predicate:** "Reconsider only inside the broader raw-worker v2 role redesign (track as F-ABI-RAW-WORKER-V2)."
- **failure_term:** `raw-worker-planner-creep`

### 2026-08-17 -- test-profile-microbench-as-keep -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Keep a product hot-path change on the strength of `crates/zerostack-harness/tests/store_cas_microbench.rs` (dev/test profile).
- **result:** REJECTED
- **evidence:** Pass 5: the focused bench-shaped test sets `keep_eligible: false` because it runs under the test profile. Keep-gate requires `release-perf`, both gates same run window, `cv_pct` reported, profile-first ≥0.1% self-time.
- **retry_condition_predicate:** "Retry only if a profiler attributes a clearly-above-noise share to SharedCas::put or atomic_write_file on a release-perf store_cas workload with cv_pct at or below 5."

### 2026-08-17 -- comprehensive-bench-93-on-mac -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Run the full 93-scenario `comprehensive-bench` matrix on this Mac and treat it as the pass-over-pass baseline.
- **result:** DEFERRED
- **evidence:** Pass 5 deliberately skipped the 93-scenario matrix on this Mac. Seed is the 12-row savings-bench self-oracle. Host noise + missing `cv_pct` would make any "win" dishonest.
- **retry_condition_predicate:** "Blocked until an rch-offloaded comprehensive-bench JSON v3 with cv_pct at or below 5 and a committed `.bench-history` refresh lands; track as F-STORE-BENCH-HISTORY."

---

## Open Candidates (queued, not yet measured)

- `profile-first-cas-put` -- expected_signal: samply self-time ≥0.1% on `SharedCas::put` under `release-perf`. Must satisfy `test-profile-microbench-as-keep` and `cv-pct-null-savings-baseline` predicates first.

## Retired Candidates Worth Flagging

- `release-perf-lying-profile` -- CLOSED in pass 5; do not re-open as a profile-honesty bug.
- `cass-unavailable-phase0` -- superseded by `cass-unavailable-phase8`.

---

*Phase 8 durable ledger. Lint with `python3 scripts/check_ledger_retry.py`.*
