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
cargo test --manifest-path conformance/Cargo.toml
~~~

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
