# AGENTS.md -- ZeroStack (hub)

Private local law (gitignored). Claude/Pi/Grok read this first. Operator override wins.

## Program (four repos, one system)

| Repo | Role |
|------|------|
| **ZeroStack** | Hub: contracts, composition, CodeMode host, gates, ledger, refs, store |
| **FSZero** | Bytes/state authority (CAS, snapshots, journals, overlays) |
| **GraphZero** | Structure authority (claims, coverage, blast, invalidation) |
| **TokenZero** | Model-facing surface (tokens, decision views, telemetry honesty) |

Engines never import each other. Hub composes them. Daemonless: one session-owned sidecar, parent-death-bound -- never a machine-wide service.

**North star:** same model, protected quality, repeated project cognition compiled away -- with receipts.

**Specs:** `~/Downloads/racc-r-handoff/` (RACC-R V1 + Q99 > V3 R5 > V2). Gold playbooks: `~/Downloads/<Repo>-GOLD-HANDOFF.md`. Claims only from receipts; labeled Q99 denominators only.

## Law (all four repos)

0. Operator override. One precise question when truly blocked.
1. No file deletion without express permission (except this cleanup session when asked).
2. Git: `main` only; conventional subjects; explicit paths; no force-push/reset-hard; no push without approval (hub rev is pin target).
3. One writer: dirty paths you did not touch = rival work -- stop and report. No claim = free.
4. Tests: RCH only, targeted only -- never full workspace cargo on this Mac.
5. Beads are memory: claim one, finish, verify, commit, close. Never batch-claim.
6. Smallest correct change. Reuse first.
7. Fail loud. No silent success, no heuristic labeled exact.

## This repo -- hub focus

**Owns:** `zero-abi`, `zero-ref`, `zero-store`, `zero-ledger`, `zero-gate`, `zero-cert`, `zero-gauge`, `zero-codemode`, `zero-testkit`, conformance contracts.

**Current program state (2026-08):**
- Raw-worker v2 + shared zero-codemode cutover largely landed and pushed.
- Aggregate CodeMode path: option honor, output budgets, public result normalization, Graph worker probe fixes on origin/main.
- Still open frontier: session sidecar (`q6am`), 1ms warm latency, machine-permit recovery, shared test library under `tests/`, store-root/CAS convergence, installer/distribution epics, RACC-R adoption residual.

**Hard gates to preserve:**
- Warm tool p50/p95 budgets; idle CPU/RSS caps; no global daemon.
- Raw worker: no planner, no nested CodeMode, no MCP catalog.
- ABI digest bumps when semantics change.

**Do not:** implement peer-engine features here; invent MCP ZeroStack tools on Grok (use CodeMode `zs`); claim without `br ready`.

## Ops defaults

```bash
br ready --json
br update <id> --claim   # only when you take work
br sync --flush-only     # before commit
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p <crate> <filter> -- --test-threads=1
zs --json -C "$PWD" fs '...'   # CodeMode, not native grep/read for local FS
```
