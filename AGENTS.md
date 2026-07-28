# ZeroStack documentation hub

This repository is the canonical public aggregation point for ZeroStack. Keep engine implementation code in the TokenZero, FSZero, and GraphZero repositories.

## Scope

- Public documentation lives in `README.md` and `docs/`.
- Reproducible measurements live in `benchmarks/`.
- Shared contracts and checks live in `conformance/`.
- Shared foundation crates live in `crates/`.
- Do not publish private engine source or private package source here.

## Documentation rules

- Describe both supported deployment modes: standard MCP adapter or CodeMode.
- State that deployments choose exactly one mode and never run both simultaneously.
- Mark TokenZero public; mark FSZero and GraphZero private and in development until their status changes.
- Use repository-relative examples. Do not add machine-specific absolute paths, credentials, personal data, or private URLs.
- Treat benchmark claims as evidence-backed only when their artifacts are committed.

## Tool routing: use the zero surface, not native file tools

This repository is served by the ZeroStack CodeMode wrapper. Agents must route work through `zero_execute` instead of native read/grep/list/shell tools, because the native tools bypass compression and return full payloads into context.

Check the wrapper first with `zerostack_status`, then route:

| Instead of | Use |
| --- | --- |
| shell / bash | `zero.token.shell(cmd)`, or `{background:true}` then `zero.token.job(id)` |
| read file | `zero.fs.compound('read', { path })` |
| grep / search | `zero.fs.compound('search', { query, path })` |
| list dir | `zero.fs.compound('list', { path })` |
| write file | `zero.fs.compound('write', { path, content })` |
| find callers, deps, impact | `zero.graph.query(surface, target)`, `zero.graph.blast(symbol)` |
| shrink a large intermediate | `zero.token.compact(data)` |

Known surface constraints, confirmed by use:

- `zero.fs.compound('search', ...)` matches **literal strings only**. Regex alternation such as `a|b` silently returns zero hits, so issue one call per term.
- `zero.fs.compound('read', ...)` has no line-range parameter. `lines` is ignored and a `#L<a>-<b>` fragment in `path` is rejected. To read part of a large file, slice it to a temp file inside the workspace first.
- Read payloads are truncated to a visible budget and the remainder is left in the result ref. Keep single reads under roughly 2 KB of expected output, or the useful lines will be hidden.
- Paths must resolve under the session root. Absolute paths outside it are refused.

## Validation

Run documentation privacy checks, then the conformance suite when its Rust toolchain is available:

~~~sh
rg -n '/Users/|/home/|BEGIN .*PRIVATE KEY|api[_-]?key|password' README.md docs AGENTS.md benchmarks conformance
python3 scripts/check_no_host_paths.py
python3 scripts/scrub_beads_export.py --check
cargo test --manifest-path conformance/Cargo.toml
~~~

Privacy gates (`check_no_host_paths.py`, `scrub_beads_export.py --check`) are
wired in `.github/workflows/ci.yml`, but that workflow is **workflow_dispatch only**
(budget: CI is deliberately not on every push). A leak fails the build **when the
workflow is dispatched**, not automatically on every commit. Agents must still run
the local checks before staging `.beads/` or path-bearing files.

<!-- BEGIN BEADS INTEGRATION v:2 tracker:br -->
## Issue Tracking with br (beads_rust)

**Note:** `br` is non-invasive and never executes git commands. After `br sync --flush-only`, you must manually run `git add .beads/ && git commit`.

### Quick Reference

```bash
br ready                # Find available work
br show <id>            # View issue details
br update <id> --claim  # Claim work
br close <id>           # Complete work
br stats                # Database overview
```

### Rules

- Use `br` for ALL task tracking. Do not use TodoWrite, TaskCreate, or markdown TODO lists.
- `br ready` is the single work-discovery entrypoint. Do not hand-roll status filters like `br list -s open`.
- Read-only inspection goes through `bv --robot-*`. Avoid bare `bv` in automated sessions.
- Use `RUST_LOG=error` for routine `br` runs to suppress dependency logs.

**Architecture in one line:** issues live in a local SQLite DB at `.beads/beads.db`; `.beads/issues.jsonl` is the git-friendly export written by `br sync --flush-only`.

**`bd` is retired in this project.** Its issues were merged into `br`, and `.beads/embeddeddolt` is legacy data. Do not run `bd`: it writes a second, divergent store that `br` cannot import, because `bd` emits `comments[].id` as a string where `br` requires an integer. The one-time reconcile lives at `scripts/reconcile_bd_to_br.py`.

The same rule applies to the TokenZero, FSZero, and GraphZero repositories: all four use `br`.

## Git: these working trees are SHARED between concurrent sessions

Several agent sessions work the same checkout of ZeroStack, TokenZero, FSZero
and GraphZero at once, and peers routinely hold large amounts of uncommitted
work. Assume anything you did not just write belongs to someone else.

**Never run destructive git on files you did not create.** No `git clean`, no
`git reset --hard`, no `git checkout -f`, and no bare or `-u` `git stash`.
`git stash push -u` sweeps up peers' untracked files, and `git rebase`
autostashes their dirty tree. Both have already come close to stranding another
session's work here.

If you need a clean tree, do not clean the shared one:

```bash
git worktree add --detach /tmp/<yourname> origin/main
```

That is the supported way to build or test against pristine `main`, and it
costs nothing. If you must stash, scope it to your own explicit paths rather
than stashing the whole tree.

**Claim atomically, and hold the claim only while you are working.** The
tracker is the lock:

```bash
br update <id> --claim          # atomic: assignee=you + status=in_progress
br coordination status          # who holds what, and how stale it is
```

`br update --claim` is a single atomic operation; setting `--status` and
`--assignee` in separate calls is not, and races another session claiming the
same bead. Run `br coordination status` before picking up work: it lists every
in-progress claim with its holder and age, and flags stale ones (`fresh` under
120 minutes, abandoned after 480).

A claim you are not actively working is worse than no claim, because it hides
the bead from `br ready` while nobody advances it. If you stop, release it:

```bash
br update <id> --status open --assignee ""
```

**An open bead does not mean the work is undone.** Before claiming, search the
history as well as the tracker:

```bash
git log --oneline -20 --all --grep=<keyword>
```

Work has already been duplicated because a fix landed on `main` while its bead
stayed open. If you fix something that has no bead, file it and close it in the
same push so the tracker matches reality.

**Claim the bead atomically, and check who else is claiming.** `br` has this
built in; use it rather than a bare `--status` edit, which can race:

```bash
br update <id> --claim               # atomic: assignee=actor + in_progress
br coordination status               # every in-progress claim, with stale evidence
```

`br coordination status` classifies each claim as fresh/stale/abandoned
(thresholds 120m/480m) and tells you whether reclaiming is allowed by policy.
Read it before assuming an in-progress bead is abandoned.

**Then claim the FILE, which is a separate problem.** `br` claims ISSUES and has
no concept of paths, so two agents holding two different, correctly claimed beads
can still land in the same file. That is exactly how the collisions in
zerostack-xyk happened, and no amount of bead hygiene prevents it. Announce the
file before you edit it:

```bash
python3 scripts/agent_lock.py list                     # what is everyone in?
python3 scripts/agent_lock.py check  <repo>/<path> --who <you>   # exit 1 if held
python3 scripts/agent_lock.py claim  <repo>/<path> --who <you> --why "<bead id>"
python3 scripts/agent_lock.py release <repo>/<path> --who <you>  # or --all-mine
```

Paths are `Repo/relative/path`, so one namespace covers all four repos from the
hub. State is `.agent-locks.json` at the hub root, gitignored, never committed.
Set `AGENT_NAME` and `--who` can be omitted.

The lock is **advisory**: it cannot stop a write, and it is not trying to. Its
job is to make an intention visible *before* the edit so a peer can pick
different work. Locks expire after 2h (`--ttl`) so a dead session cannot wedge a
file forever; a stale lock is reported `[STALE, breakable]` and can be taken
without `--force`. Release when you are done, and prefer
`release --all-mine` at the end of a work item over leaving locks lying around.

If a file you need is held, do not just wait or barge in: mail the holder. The
common case is that they are elsewhere in the file and a quick split is cheaper
than either of you blocking.

**Push small, often, and rebased onto `main`.** Commits parked on a private
branch are invisible to every other session, which is precisely what causes
duplicate effort. Pull before you start an item, not just before you push, and
keep one logical change per commit. Announce pushes to peers over Agent Mail:
repo, sha, one line. Do not rewrite pushed history without saying so first.

## Session Completion

This protocol is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - `br create` anything that needs follow-up
2. **Run quality gates** (if code changed) - tests, linters, builds
3. **Update issue status** - close finished work, update in-progress items
4. **Export, scrub, and stage:**
   ```bash
   br sync --flush-only
   python3 scripts/scrub_beads_export.py
   git add .beads/
   git commit -m "sync beads"
   ```
   Do not push without an explicit request.

   The scrub is required, not optional. `br` stamps every issue record with
   `source_repo_path`, an absolute host path, and 0.2.16 has no config knob to
   omit or relativize it. `.beads/issues.jsonl` is tracked in this public repo,
   so an unscrubbed export publishes the author's username and directory layout
   on every issue. The scrub rewrites that field to the repo name and relativizes
   host paths in descriptions and comments; it is idempotent and safe to re-run.
   The other three repos use the same script from here.
5. **Hand off** - summarize changes, validation, issue status, and any blocked step

**Critical rules:**
- Explicit user or orchestrator instructions override this block.
- Do not commit or push without clear authority or a current user request.
- If a required sync is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- bv-agent-instructions-v3 -->

---

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`) for issue tracking and [beads_viewer](https://github.com/Dicklesworthstone/beads_viewer) (`bv`) for graph-aware triage. Issues are stored in `.beads/` and tracked in git. Current `br` workspaces normally export `.beads/issues.jsonl`; older `bd`/legacy workspaces may use `.beads/beads.jsonl`. `bv` auto-discovers the supported JSONL files, so agents should use `br`/`bv` commands instead of hard-coding a single filename.

### Using bv as an AI sidecar

bv is a graph-aware triage engine for Beads projects. Instead of parsing .beads/issues.jsonl / .beads/beads.jsonl directly or hallucinating graph traversal, use robot flags for deterministic, dependency-aware outputs with precomputed metrics (PageRank, betweenness, critical path, cycles, HITS, eigenvector, k-core).

**Scope boundary:** bv handles *what to work on* (triage, priority, planning). `br` handles creating, modifying, and closing beads.

**CRITICAL: Use ONLY --robot-* flags. Bare bv launches an interactive TUI that blocks your session.**

#### The Workflow: Start With Triage

**`bv --robot-triage` is your single entry point.** It returns everything you need in one call:
- `quick_ref`: at-a-glance counts + top 3 picks
- `recommendations`: ranked actionable items with scores, reasons, unblock info
- `quick_wins`: low-effort high-impact items
- `blockers_to_clear`: items that unblock the most downstream work
- `project_health`: status/type/priority distributions, graph metrics
- `commands`: copy-paste shell commands for next steps

```bash
bv --robot-triage        # THE MEGA-COMMAND: start here
bv --robot-next          # Minimal: just the single top pick + claim command

# Token-optimized output (TOON) for lower LLM context usage:
bv --robot-triage --format toon
```

Before claiming, verify current state with `br show <id> --json` or `br ready --json`. `recommendations` can include graph-important blocked or assigned work; only `quick_ref.top_picks` and non-empty `claim_command` fields represent claimable work.

#### Other bv Commands

| Command | Returns |
|---------|---------|
| `--robot-plan` | Parallel execution tracks with unblocks lists |
| `--robot-priority` | Priority misalignment detection with confidence |
| `--robot-insights` | Full metrics: PageRank, betweenness, HITS, eigenvector, critical path, cycles, k-core |
| `--robot-alerts` | Stale issues, blocking cascades, priority mismatches |
| `--robot-suggest` | Hygiene: duplicates, missing deps, label suggestions, cycle breaks |
| `--robot-diff --diff-since <ref>` | Changes since ref: new/closed/modified issues |
| `--robot-graph [--graph-format=json\|dot\|mermaid]` | Dependency graph export |

#### Scoping & Filtering

```bash
bv --robot-plan --label backend              # Scope to label's subgraph
bv --robot-insights --as-of HEAD~30          # Historical point-in-time
bv --recipe actionable --robot-plan          # Pre-filter: ready to work (no blockers)
bv --recipe high-impact --robot-triage       # Pre-filter: top PageRank scores
```

### br Commands for Issue Management

```bash
br ready --json                       # Show issues ready to work (no blockers)
br list --status=open --json          # All open issues
br show <id> --json                   # Full issue details with dependencies
br create --title="..." --type=task --priority=2 --json
br update <id> --status=in_progress --json
br close <id> --reason="Completed" --json
br close <id1> <id2> --reason="Completed" --json
br sync --flush-only                  # Export DB to JSONL after Beads mutations
```

### Workflow Pattern

1. **Triage**: Run `bv --robot-triage` to find the highest-impact actionable work
2. **Claim**: Use `br update <id> --status=in_progress --json`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id> --reason="Completed" --json`
5. **Sync**: Run `br sync --flush-only` after Beads mutations so the JSONL export is current

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready --json` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers 0-4, not words)
- **Types**: task, bug, feature, epic, chore, docs, question
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Git Policy

`br` never commits or pushes. Follow this repository's own git instructions before staging, committing, or pushing. If the repository says "commit only when asked," that rule overrides any generic workflow advice.

<!-- end-bv-agent-instructions -->


## Local conformance evidence (Option B)

conformance/reports/ is ignored private runtime output, not a durable repository or CI attestation. CI and clean clones do not verify report freshness. Validate an intentionally supplied local index with python3 conformance/scripts/check_freshness.py <explicit-index.json>; the validator never creates evidence. Durable publication requires a separate signed, scrubbed, provenance-pinned, reviewed flow with committed artifacts and CI signature/freshness verification.
