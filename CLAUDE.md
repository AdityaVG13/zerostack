# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

## Tool routing: use the zero surface, not native file tools

This repository is served by the ZeroStack CodeMode wrapper. Route work through `zero_execute` instead of native read/grep/list/shell tools, which bypass compression and pull full payloads into context. Probe with `zerostack_status` first.

| Instead of | Use |
| --- | --- |
| shell / bash | `zero.token.shell(cmd)`, or `{background:true}` then `zero.token.job(id)` |
| read file | `zero.fs.compound('read', { path })` |
| grep / search | `zero.fs.compound('search', { query, path })` |
| list dir | `zero.fs.compound('list', { path })` |
| write file | `zero.fs.compound('write', { path, content })` |
| find callers, deps, impact | `zero.graph.query(surface, target)`, `zero.graph.blast(symbol)` |
| shrink a large intermediate | `zero.token.compact(data)` |

Surface constraints confirmed by use: search is literal-only (regex alternation returns zero hits), `read` has no working line-range parameter, read payloads truncate to a visible budget with the rest left in the result ref, and paths must resolve under the session root. See `AGENTS.md` for detail.

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

**`bd` is retired in this project.** Its issues were merged into `br`, and `.beads/embeddeddolt` is legacy data. Do not run `bd`: it writes a second, divergent store that `br` cannot import, because `bd` emits `comments[].id` as a string where `br` requires an integer.

## Session Completion

This protocol is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - `br create` anything that needs follow-up
2. **Run quality gates** (if code changed) - tests, linters, builds
3. **Update issue status** - close finished work, update in-progress items
4. **Export and stage:**
   ```bash
   br sync --flush-only
   git add .beads/
   git commit -m "sync beads"
   ```
   Do not push without an explicit request.
5. **Hand off** - summarize changes, validation, issue status, and any blocked step

**Critical rules:**
- Explicit user or orchestrator instructions override this block.
- Do not commit or push without clear authority or a current user request.
- If a required sync is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->


## Build & Test

_Add your build and test commands here_

```bash
# Example:
# npm install
# npm test
```

## Architecture Overview

_Add a brief overview of your project architecture_

## Conventions & Patterns

_Add your project-specific conventions here_
