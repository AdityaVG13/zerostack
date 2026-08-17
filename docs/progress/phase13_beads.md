# Phase 13 beads -- remaining remediations

**Date:** 2026-08-17
**HEAD intent:** `phase13: remaining remediations as beads`
**Mode:** beads handoff only. No product remediations implemented. No claims.
**Dashboard (Phase 12):** `effective=0.952282` `strict=0.940573` `gate=red` (honest)

Beads live in ZeroStack `.beads/` (gitignored). This file is the tracked handoff.
Created via `br` only. `br sync --flush-only` after mutations. `br dep cycles` empty.
`bv --robot-insights` `cycle_count=0`. No bead set `in_progress`.

## What was beaded

Remaining CONFIRMED_GAP plus later-scored in-hub items from Phase 12. Already-closed
AUTO-FIXES are not re-filed.

### In graph

| Gap | Rewrite beaded | Score | Where |
|---|---|---:|---|
| SURF-0006 / F-CI-PR-GATES | DSR `quality` targeted `cargo test -p` | 8.0 | operator `~/.config/dsr/repos.yaml` |
| PERF-0001 / OPEN-0015 | rch savings-bench `cv_pct` n=10 | 3.0 | rch measurement |
| PERF-0003 | install cass + 60d mine | 7.5 | operator host |
| SURF-0013 / F-STORE-QUARANTINE-REAP | quarantine + reap unit test | 6.0 | in-repo `zero-store` |
| CONF-0005 / SPEC-HUB-005 | ABI digest mutation test | 6.0 | in-repo `zero-abi` / harness |
| F-MIRI-NARROW | rch `miri test -p zero-ref` green | 4.5 | DSR/rch |
| F-REF-ERROR-TAXONOMY | reserved classes stay reserved | 10.0 | standing; `zero-ref` |
| F-ZSX-Q99-REPORT residual | stay honest partial | -- | standing; no fake accounting |
| F-CONF-HARNESS | document no in-repo CLI | 10.0 | standing; CONTRACT §8 |

### Explicitly not beaded

- Engine ClampEnd lockstep (`F-REF-ENGINE-ADOPTION-LOCKSTEP`)
- TokenZero Exact emission (`[SPEC-HON-001]`)
- In-repo conformance CLI
- GitHub Actions `cargo test --workspace` / any automatic GH test job
- Fat-LTO flip (`PERF-0002`)
- Rival dirty `crates/zsx-core/src/fszero.rs` work

## Bead IDs created (32 new)

None of these IDs existed before this pass. No remaining-gap beads were reused.

### Epic

| ID | Title | Type | P |
|---|---|---|---:|
| `zerostack-gauntlet-p13-remaining-0muh` | gauntlet: phase13 remaining remediations | epic | 1 |

### Remediations (blocked on test + bench + doc)

| ID | Gap | Title | P |
|---|---|---|---:|
| `zerostack-gauntlet-surf-0006-1nlp` | SURF-0006 | wire dsr quality targeted cargo test -p | 1 |
| `zerostack-gauntlet-perf-0003-049q` | PERF-0003 | install cass and mine 60d | 1 |
| `zerostack-gauntlet-perf-0001-fq2l` | PERF-0001 | record rch savings-bench cv_pct n=10 | 2 |
| `zerostack-gauntlet-surf-0013-7yad` | SURF-0013 | quarantine + reap unit test | 2 |
| `zerostack-gauntlet-conf-0005-pvmp` | CONF-0005 | ABI digest mutation test | 2 |
| `zerostack-gauntlet-miri-narrow-hxsh` | F-MIRI-NARROW | rch-green miri test -p zero-ref | 2 |
| `zerostack-gauntlet-ref-error-taxonomy-u74o` | F-REF-ERROR-TAXONOMY | keep reserved classes reserved | 3 |
| `zerostack-gauntlet-zsx-q99-v39v` | F-ZSX-Q99-REPORT | residual stay honest partial | 3 |
| `zerostack-gauntlet-conf-harness-eh8k` | F-CONF-HARNESS | document no in-repo conformance CLI | 3 |

### Evidence beads

| ID | Kind | Title |
|---|---|---|
| `zerostack-gauntlet-surf-0006-test-ni2s` | test | prove dsr quality runs targeted cargo test -p |
| `zerostack-gauntlet-surf-0006-bench-oxuh` | bench | targeted -p checks stay host-safe |
| `zerostack-gauntlet-surf-0006-doc-3psu` | doc | F-CI-PR-GATES present only after DSR evidence |
| `zerostack-gauntlet-perf-0001-test-dovy` | test | ratchet refuses invented cv_pct |
| `zerostack-gauntlet-perf-0001-bench-aq5p` | bench | ten rch release-perf savings-bench repeats |
| `zerostack-gauntlet-perf-0001-doc-6pce` | doc | ledger the cv_pct result without a keep-claim |
| `zerostack-gauntlet-perf-0003-test-m79z` | test | cass health --robot is green |
| `zerostack-gauntlet-perf-0003-bench-hkjg` | bench | mine 60d cass vs git-log fallback completeness |
| `zerostack-gauntlet-perf-0003-doc-v5nb` | doc | replace cass-missing blocker in the ledger |
| `zerostack-gauntlet-surf-0013-test-fije` | test | put reaps stale tmp and quarantine digest mismatch |
| `zerostack-gauntlet-surf-0013-doc-bmd7` | doc | F-STORE-QUARANTINE-REAP present after tests |
| `zerostack-gauntlet-conf-0005-test-dpcf` | test | mutate C-23/24/26 pin fails if digest unchanged |
| `zerostack-gauntlet-conf-0005-doc-k7r9` | doc | SPEC-HUB-005 verified after mutation test |
| `zerostack-gauntlet-miri-narrow-test-t9gz` | test | rch cargo +nightly miri test -p zero-ref green |
| `zerostack-gauntlet-miri-narrow-doc-h9gf` | doc | present only after rch log |
| `zerostack-gauntlet-ref-error-taxonomy-test-z4gr` | test | parser never emits reserved classes |
| `zerostack-gauntlet-ref-error-taxonomy-doc-t8r4` | doc | stay partial until store paths construct reserved |
| `zerostack-gauntlet-zsx-q99-test-fkt1` | test | empty-window unavailable stays green |
| `zerostack-gauntlet-zsx-q99-doc-9qy6` | doc | residual stays honest partial |
| `zerostack-gauntlet-conf-harness-test-gkkl` | test | no in-repo conformance CLI or second catalog |
| `zerostack-gauntlet-conf-harness-doc-b8i7` | doc | harness-as-library stays partial |
| `zerostack-gauntlet-shared-no-keep-h986` | bench | SHARED savings-bench seed stays ineligible |

## Dependency graph

Direction: `br dep add <child> <depends-on>` -- child is blocked until depends-on closes.

```
                    ┌─────────────────────────────────────┐
                    │  p13-remaining-0muh  (epic)         │
                    └─────────────────────────────────────┘
                       ▲  ▲  ▲  ▲  ▲  ▲  ▲  ▲  ▲
                       │  │  │  │  │  │  │  │  │
         SURF-0006-1nlp┘  │  │  │  │  │  │  │  └─ conf-harness-eh8k
         PERF-0003-049q───┘  │  │  │  │  │  └──── zsx-q99-v39v
         PERF-0001-fq2l──────┘  │  │  │  └─────── ref-error-taxonomy-u74o
         SURF-0013-7yad─────────┘  │  └────────── miri-narrow-hxsh
         CONF-0005-pvmp────────────┘

Each remediation depends on test + bench + doc:

SURF-0006-1nlp  → test-ni2s, bench-oxuh, doc-3psu
PERF-0001-fq2l  → test-dovy, bench-aq5p, doc-6pce
PERF-0003-049q  → test-m79z, bench-hkjg, doc-v5nb
SURF-0013-7yad  → test-fije, shared-no-keep-h986, doc-bmd7
CONF-0005-pvmp  → test-dpcf, shared-no-keep-h986, doc-k7r9
miri-narrow-hxsh→ test-t9gz, shared-no-keep-h986, doc-h9gf
taxonomy-u74o   → test-z4gr, shared-no-keep-h986, doc-t8r4
zsx-q99-v39v    → test-fkt1, shared-no-keep-h986, doc-9qy6
conf-harness-eh8k → test-gkkl, shared-no-keep-h986, doc-b8i7
```

Shared bench `zerostack-gauntlet-shared-no-keep-h986` is the honesty gate for
non-measurement remediations: do not invent `cv_pct` or flip `keep_eligible`.

## Validation

```
$ br dep cycles
✓ No dependency cycles detected.

$ br dep cycles --json
{"cycles":[],"count":0,"active_count":0,"archived_closed_count":0,"total_count":0,...}

$ bv --robot-insights  |  advanced_insights.cycle_break
{"cycle_count":0,"advisory":"No cycles detected - dependency graph is a proper DAG."}
```

Every `kind:remediation` bead has `kind:test` + `kind:bench` + `kind:doc` deps.
All 32 new beads are `status=open`. None claimed.

## `br ready` -- Phase 13 subset (22)

Evidence beads are ready first (TDD / proof-before-close). Remediations stay
blocked until those close. Do not claim from this handoff.

P1:

- `zerostack-gauntlet-surf-0006-test-ni2s`
- `zerostack-gauntlet-surf-0006-bench-oxuh`
- `zerostack-gauntlet-surf-0006-doc-3psu`
- `zerostack-gauntlet-perf-0003-test-m79z`
- `zerostack-gauntlet-perf-0003-bench-hkjg`
- `zerostack-gauntlet-perf-0003-doc-v5nb`

P2:

- `zerostack-gauntlet-shared-no-keep-h986`
- `zerostack-gauntlet-perf-0001-test-dovy`
- `zerostack-gauntlet-perf-0001-bench-aq5p`
- `zerostack-gauntlet-perf-0001-doc-6pce`
- `zerostack-gauntlet-surf-0013-test-fije`
- `zerostack-gauntlet-surf-0013-doc-bmd7`
- `zerostack-gauntlet-conf-0005-test-dpcf`
- `zerostack-gauntlet-conf-0005-doc-k7r9`
- `zerostack-gauntlet-miri-narrow-test-t9gz`
- `zerostack-gauntlet-miri-narrow-doc-h9gf`

P3:

- `zerostack-gauntlet-ref-error-taxonomy-test-z4gr`
- `zerostack-gauntlet-ref-error-taxonomy-doc-t8r4`
- `zerostack-gauntlet-zsx-q99-test-fkt1`
- `zerostack-gauntlet-zsx-q99-doc-9qy6`
- `zerostack-gauntlet-conf-harness-test-gkkl`
- `zerostack-gauntlet-conf-harness-doc-b8i7`

## Reused vs created

| Class | Action |
|---|---|
| Remaining SURF-0006 / PERF-0001 / PERF-0003 / SURF-0013 / CONF-0005 / F-MIRI-NARROW / F-REF-ERROR-TAXONOMY / F-ZSX-Q99-REPORT / F-CONF-HARNESS | **Created** (no prior bead) |
| Earlier gauntlet-p0..p12 / K0 / cutover beads | **Left untouched** (different work) |
| Closed AUTO-FIXES (SURF-0010/0011, IDEA-0012/0015, ADV-0003) | **Not re-filed** |

## How a later agent should close a cluster

1. `br ready` -- pick an evidence bead. One writer. Do not batch-claim.
2. Implement only that bead. Targeted rch/DSR only. No GH cargo test job.
3. `br close <id> --reason "..."` with the invocation output.
4. After test + bench + doc are closed, the remediation becomes ready.
5. Close the remediation with matrix/ledger evidence. Never paint gate green
   by rounding partials up.
6. `br sync --flush-only`. Do not force-add `.beads/`.

## Standing honesty (do not invert)

- Partial never rounds up.
- Do not invent `cv_pct`.
- Do not fabricate cass hits.
- Do not implement a conformance CLI.
- Do not fake `WorkerTokenAccountingV1`.
- Do not construct reserved `ZeroRefErrorClass` variants on fake store paths
  in `zero-ref`.
- Do not touch rival dirty `fszero.rs`.
