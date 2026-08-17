---
name: PARITY_RUNBOOK
schema_version: gauntlet.parity-runbook.v1
generated_at_utc: "2026-08-17T14:10:00Z"
run_id: "20260817-141000-d4caada"
port_name: "ZeroStack"
project_class: "greenfield-rust"
reference_name: "spec-as-oracle"
final_artifact_tier: "public-release"
---

# PARITY_RUNBOOK -- Keeping ZeroStack honest (spec-as-oracle)

This is the maintenance-mode operations manual. Read it when (a) onboarding, (b) wiring gates, (c) responding to a red dashboard or a suspected keep. It is the durable counterpart to `docs/progress/FINAL_GAUNTLET_REPORT.md` (the Phase 16 snapshot). That report is **NOT CERTIFIED**. This runbook exists so later agents do not lie the port green.

Class: **Greenfield-Rust-class**. Oracle is spec / property / self / round-trip / external-tool -- not a pinned upstream binary.

## Standing laws (read before any command)

1. **Never** `cargo test --workspace` on this Mac. Never add a GitHub Actions job that does it.
2. Heavy cargo goes through **rch** only:

   ```bash
   rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
     cargo test -p <one-crate> -- --test-threads=1
   ```

3. Do **not** touch the rival dirty tree: `crates/zsx-core/src/fszero.rs`, `docs/codemode.md`, `tests/unit/zsx-core/fszero_tests.rs`, `.zsx_patch.diff`.
4. Dashboard **red is honest**. Partial never rounds up. Excluded is strict-100% debt.
5. `cv_pct` null is unknown. `cv_pct > 5` is **noise**, not a keep.
6. Bless goldens only with `scripts/bless-golden.sh --i-am-the-operator`.
7. Subject label is `zerostack`. It must never equal an Oracle label.
8. Both-error is agreement regardless of message. One-error-one-OK is a hard failure.
9. If `cass` is missing, record a blocker. Do not silently skip the 60-day mine.
10. Fat-LTO is a **named waiver** in `Cargo.toml`. Do not flip it to claim a win.

## 1. Honesty scripts to run (the five Python gates)

These are the in-repo gates. Run them locally (no rch) after any FeatureUniverse, golden, bench-history, or ledger edit. Phase 16 re-ran all five; paste is in `FINAL_GAUNTLET_REPORT.md` §10.

```bash
python3 scripts/check_feature_universe_weights.py
# expect: feature-universe ok: features=77 present=67 partial=7 missing=0 excluded=3 weight_sum=1.000000000000

python3 scripts/check_feature_coverage_dashboard.py
# expect: effective=0.952282 strict=0.940573 gate=red
# EXIT=0 means the *script* is healthy. gate=red means the *product* is not release-green.

python3 scripts/check_golden_integrity.py
# expect: golden-integrity ok: checksums=12 artifacts=11 tier1=5 schema=1.0.0
# This script never blesses.

python3 scripts/check_bench_history.py --self-test
python3 scripts/check_bench_history.py
# expect: cv_pct is null (unknown; not a win); verdict pass (no regression)

python3 scripts/check_ledger_retry.py
# expect: ledger-retry ok: files=3
```

Also run, when the change can leak a host path:

```bash
scripts/check-portability.sh
```

### What the skill-template CI scripts are **not**

This repo does **not** ship `scripts/compute-parity-score.sh`, `scripts/apply-ratchet.sh`, `scripts/run-bench-matrix.sh`, `scripts/run-conformance-suite.sh`, `scripts/compute-feature-coverage.sh`, `scripts/convergence-tracker.sh`, or `scripts/bead-graph-validator.sh`. Those names live in the gauntlet skill. Do not invent wrappers to look certified. The in-repo equivalents are the five Python scripts above plus targeted `rch` crate tests.

### 1.1 FeatureUniverse + dashboard (surface release-gate)

Wired today in `.github/workflows/ci.yml` job `feature-universe` (`workflow_dispatch` only):

```yaml
- run: python3 scripts/check_feature_universe_weights.py
- run: python3 scripts/check_feature_coverage_dashboard.py
- run: python3 scripts/check_bench_history.py
```

Weight policy is **global-sum-1.0** (`[SPEC-FU-003]`), not per-family 1.0. The loader rejects a sum that is not 1.0. `truncate_score` is 6 decimal places. Partial contribution is 0.5. `strict_coverage` treats excluded as debt. A family is never `full` while it still has partial/missing/excluded.

**Do not** flip `release_gate_verdict` to green in the JSON by hand. The dashboard script is the gate.

### 1.2 Goldens

```bash
python3 scripts/check_golden_integrity.py
```

Recapture (operator only):

```bash
scripts/bless-golden.sh --i-am-the-operator
# without the flag the script exits 2 and prints "Refusing to bless goldens."
```

CI and agents must not recapture to make a red test green.

### 1.3 Bench-history ratchet

Baseline file: `.bench-history/savings-bench.latest.json` (12-row self-oracle seed; `keep_eligible=false`; `cv_pct=null`).

Gates in the seed (`ci_regression_gate`): primary −3%, geomean −5%, per-category −10%, p90 −15%, throughput −5%. Absent fields are **not invented** and **not gated**.

A keep additionally requires: `release-perf`, `RUSTFLAGS="-C force-frame-pointers=yes"`, both focused and broad JSON from the same git state / same `target/` / same machine / same minute, `cv_pct` reported and `<= 5`, and a profile frame ≥0.1% self-time.

### 1.4 Ledger lint

```bash
python3 scripts/check_ledger_retry.py
```

Three files: `docs/progress/perf-negative-results.md`, `conformance-negative-results.md`, `surface-deferrals.md`. Every entry needs a retry-condition predicate in one of the eight forms (section 8). Forbidden phrases: later, TBD, TODO, FIXME, "if it seems important", "we should revisit", "tracked elsewhere", and the rest of the vocabulary block in those files.

### 1.5 Targeted cargo (rch, never workspace)

```bash
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo test -p zerostack-harness -- --test-threads=1

rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo test -p zero-ref -- --test-threads=1

rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo clippy -p zerostack-harness --all-targets --no-deps -- -D warnings

rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json
```

Preflight is **not** required to be certifying-green while gitignored `AGENTS.md` is advisory. `certifying == (aggregate_outcome == green)` must still hold. Do not bless a moving gitignored hash to force green.

### 1.6 E-process / BOCPD / fault-VFS / crash-boundary / flake / beads / convergence

| Gate the skill wants | What this repo actually has |
|---|---|
| E-process Ville 1/α | harness `eprocess` module + `cargo test -p zerostack-harness`; no separate CI job |
| BOCPD `Stable` | **not run** (Phase 15 was narrow). Do not claim Stable. |
| Fault-VFS budget | `crates/zerostack-harness` fault + crash tests (5 crash_oracle) |
| Crash-boundary coverage | same crate; named boundaries in crash oracle |
| cv_pct flake budget | `check_bench_history.py` treats null or `>5` as noise |
| Bead-graph validator | `br dep cycles` (Phase 13: empty). `.beads/` is gitignored |
| Convergence-tracker | **no in-repo script**; 10-round bench loop was never started |

### 1.7 What not to wire

```yaml
# DO NOT add this
- run: cargo test --workspace
# DO NOT add an automatic push/PR cargo test job that hammers GH + host
```

`F-CI-PR-GATES` stays **partial** until DSR (`dsr quality --tool zerostack`) grows targeted `-p` checks. That config lives in `~/.config/dsr/repos.yaml` (out of repo). As of 2026-08-17: `No quality checks configured for zerostack` (`OPEN-0014` / `NO_EVIDENCE`).

## 2. Snapshots to Keep Green

Regenerate ONLY when the underlying contract changes; never to make a red test green.

| Path | Regenerate command | Discipline |
|---|---|---|
| `crates/zerostack-harness/tests/snapshots/golden_invariants__phase4_logical_counts.snap` | `rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" cargo test -p zerostack-harness --test golden_invariants -- --test-threads=1` then review the insta diff | Logical counts must match **live** FeatureUniverse / SPEC-TAGS / CONTRACT / fixture dumps. Phase 14 found a stale pin (present=66/partial=8 after the matrix moved to 67/7). Do not hardcode catalog counts. |
| `conformance/golden/**` + `checksums.sha256` + `manifest.v1.json` | `scripts/bless-golden.sh --i-am-the-operator` | Operator only. Integrity script recaptures in memory and compares; it does not write. |

There is no planner / VDBE / RESP / OpenAPI / JIT insta family in this hub.

## 3. Fuzz Corpora to Preserve

| Directory | Size (named seeds) | Last minimization | Regeneration cost |
|---|---|---|---|
| `fuzz/corpus/zeroref_parse/valid_blob` | 1 named seed | 2026-08-17 Phase 11 | minutes to re-seed from untrusted-bytes vectors |
| `fuzz/corpus/abi_frame_decode/handshake.json` | 1 named seed | 2026-08-17 Phase 11 | minutes |
| `fuzz/corpus/*/[0-9a-f]*` | generated; **gitignored** | n/a | 30s smoke on 2026-08-17: `zeroref_parse` 28,941,563 execs / 0 crashes; `abi_frame_decode` 2,477,542 execs / 0 crashes. **Not a 24h campaign.** |

```bash
cargo +nightly fuzz list
# expect: zeroref_parse, abi_frame_decode

# smoke only -- do not claim 24h
cargo +nightly fuzz run zeroref_parse -- -max_total_time=30
```

Hex corpus files race rch rsync (`file has vanished`). That is why they are gitignored. Not a crate bug.

## 4. `// SAFETY:` Template

Every `unsafe` block must carry a comment that names invariant, precondition, postcondition, and witness. `zero-ref` is `#![forbid(unsafe_code)]` -- do not add unsafe there.

Paste-ready:

```rust
// SAFETY:
// - Invariant: <the invariant being upheld>
// - Precondition: <what the caller must guarantee>
// - Postcondition: <what this block establishes>
// - Witness: <test / fuzz / miri run that exercises it>
unsafe { /* … */ }
```

Real example from this tree (`crates/zerostack-machine-permit/src/lib.rs`):

```rust
// SAFETY: geteuid has no preconditions, reads process credentials only, and
// does not retain pointers or mutate Rust-managed memory.
unsafe { libc::geteuid() }
```

Neighboring inotify/kqueue blocks in the same file are the other live `unsafe` surface. Prefer extending those comments to the four-line form above when you touch them. Do not add new `unsafe` to close a FeatureUniverse row.

## 5. Clippy Lint Group Minimum

Today: CI `lint` job runs `cargo clippy --workspace -- -D warnings` on `workflow_dispatch`. Harness soak used:

```bash
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo clippy -p zerostack-harness --all-targets --no-deps -- -D warnings
```

Recommended workspace lints (do not bulk-allow to paint green):

```toml
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "forbid"

[workspace.lints.clippy]
pedantic = "warn"
missing_safety_doc = "deny"
undocumented_unsafe_blocks = "deny"
```

Greenfield extras that match this hub: do not treat estimates as Exact; do not `float_cmp` a token ratio into a keep.

## 6. AGENTS.md Mandate Paragraph

`AGENTS.md` is **gitignored**. The tracked copy is `docs/progress/AGENTS-MANDATE.md`. Do not use the gitignored file as FeatureUniverse evidence (`agents-md-as-feature-evidence` REJECTED). Preflight does **not** certify its hash.

Tracked mandate already contains the ledger-grep + cass-mine + recent-commits rules. Project failure terms:

`mcp-late-ok-salvage`, `raw-worker-planner-creep`, `daemon-install`, `host-path-leak`, `savingsbytes-as-tokens`, `engine-import-cycle`, `cargo-target-dir-suffix`, `rival-dirty-tree`, `clamp-end-vs-reject`, `commit-race-mislabel`, `estimate-labeled-exact`, `fszero-fail-closed`.

If `cass` is not on PATH (current host: **missing**):

```bash
command -v cass || echo "CASS_MISSING -- record a blocker; mine git log --since='60 days ago'"
```

Do not fabricate cass hits.

## 7. Negative-Ledger Format

| Field | Required? | Allowed values |
|---|---|---|
| `date` | yes | ISO 8601 |
| `hypothesis` | yes | what the agent thought would help |
| `result` | yes | `REJECTED` / `DEFERRED` / `CLOSED` |
| `evidence` | yes | paths, SHAs, commands, numbers |
| `retry_condition_predicate` | yes | one of the eight forms |

Forbidden inside the predicate and the entry body (outside the vocabulary block): later, in the future, down the road, if it seems important, we should revisit, tracked elsewhere, TBD, TODO, FIXME, maybe, eventually, when we have time, if circumstances change, future work, might be worth trying, someone should look at this, interesting direction, worth exploring.

Three verbatim samples from **this** project's ledgers:

```
### 2026-08-17 -- cv-pct-null-savings-baseline -- REJECTED
- retry_condition_predicate: "Retry only if this workload class exhibits measurable cv_pct below 5.0 on a replicated `release-perf` savings-bench run with keep_eligible true."

### 2026-08-17 -- cass-unavailable-phase8 -- DEFERRED
- retry_condition_predicate: "Blocked until cass lands on PATH (`command -v cass` succeeds and `cass health --robot` is green); track as cass-on-path."

### 2026-08-17 -- invent-second-conformance-catalog -- REJECTED
- retry_condition_predicate: "Reconsider only inside the broader CONTRACT.md §8 redesign (track as F-CONF-HARNESS)."
```

## 8. Retry-Condition Vocabulary

The eight forms (`scripts/check_ledger_retry.py` enforces them):

1. `"Retry only if a profiler attributes a clearly-above-noise share to <COUNTER> on <WORKLOAD_SHAPE>."`
2. `"Reconsider only inside the broader <X> redesign (track as <beads_id>)."`
3. `"Worth reconsidering when <GATE> crosses <THRESHOLD>."`
4. `"Not worth retrying as a standalone patch."`
5. `"Do not retry from a cold read; use comprehensive-bench attribution instead."`
6. `"Retry condition not applicable -- the gain is structural, not numerical."`
7. `"Retry only if this workload class exhibits measurable <PROPERTY> below <THRESHOLD>."`
8. `"Blocked until <ARCHITECTURAL_DEPENDENCY> lands; track as <beads_id>."`

## 9. EngineIdentity + both-error + when to escalate

### EngineIdentity rules

Source: `crates/zerostack-harness/src/engine_identity.rs`.

- Subject label is always `zerostack` (`SUBJECT_IDENTITY_LABEL`).
- Allowed Oracle labels: `spec-v1`, `property-suite-v1`, `prior-commit-<sha>`, `round-trip`, `miri`, `clippy`.
- Fields are **private**. `EngineIdentity::oracle("zerostack")` **panics**. Empty oracle label panics. Disallowed labels panic.
- Comparator entry asserts Subject ≠ Oracle (`assert_identities` / `assert_subject_ne_oracle`).
- This type is **not** `zero_abi::raw_worker::EngineIdentity` (fz / gz / tz dispatch).
- Phase 14 finding: public fields + envelope skipping the guard was a self-compare hole. That is fixed. Do not re-open the fields.

### Both-error rule

Source: `crates/zerostack-harness/src/oracle.rs`.

```text
Both-error = agreement regardless of message.
One-error-one-OK = hard failure.
```

Agreement-by-error-message-string is a named anti-pattern. `INV-BOTH-ERROR` lives in `conformance/contracts/invariant_catalog.toml`.

### Escalate when

- Dashboard script reports a histogram change you did not intend, or someone proposes flipping `gate=red` without closing rows.
- FeatureUniverse loader rejects `sum(weights) != 1.0`.
- `check_golden_integrity.py` goes red (do not bless unless you are the operator).
- `cv_pct > 5` on three consecutive **release-perf** savings-bench repeats (then quarantine; do not keep).
- A new `TrueDivergence` (emit a `FailureBundle` with `/failure/first_divergence`; do not compare error strings).
- `cass` is still missing and someone wants to start a perf campaign anyway -- record the blocker first.
- BOCPD / 24h fuzz / rch Miri are still absent -- do not claim they ran.
- Rival dirty files show up in `git status` after your `cargo fmt`. Restore them. Do not stage them.

Escalation = open / claim the matching Phase 13 bead (`br ready`, one writer). Do not batch-claim. Do not paint the dashboard.

## 10. Resuming the Gauntlet

When `origin/main` has moved and you want another honest pass (not a certification ritual):

```bash
# 1. Honesty scripts (local)
python3 scripts/check_feature_universe_weights.py
python3 scripts/check_feature_coverage_dashboard.py
python3 scripts/check_golden_integrity.py
python3 scripts/check_bench_history.py --self-test
python3 scripts/check_bench_history.py
python3 scripts/check_ledger_retry.py

# 2. Targeted rch tests (never --workspace)
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo test -p zerostack-harness -- --test-threads=1
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo test -p zero-ref -- --test-threads=1

# 3. Optional doctor (advisory AGENTS.md is expected)
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" \
  cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json

# 4. Optional narrow soaks (do not claim 24h)
scripts/run_miri_narrow.sh          # host today; rch Miri is missing
cargo +nightly fuzz run zeroref_parse -- -max_total_time=30
```

If any Python gate goes non-zero, stop. If the dashboard is still `gate=red`, that is the correct answer until the 7 partials and 3 excluded rows change for real.

A **true** Phase 5-10 convergence loop (10 comprehensive-bench rounds, 2 consecutive clean, 0 OPEN) has **not** been started. Do not write `reports/convergence_tracker.json` that pretends otherwise. If you start it, offload the bench to rch (`OPEN-0016`) and refuse a Mac 93-row matrix (`comprehensive-bench-93-on-mac`).

Full per-phase playbook lives in the skill (`references/PHASES.md`). In-repo progress files live in `docs/progress/`. Fresh-eyes notes: `docs/progress/phase14_fresh_eyes.md`. Soak scorecard: `docs/progress/soak_round_0.json`.

---

*Generated by the running-the-gauntlet-on-your-rust-port skill at Phase 16. Living document -- update on every gauntlet pass. Public-release, not a certification bundle.*
