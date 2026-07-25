# CLAUDE.md — ZeroStack

Read and follow @AGENTS.md. It is the operating manual for this repo: tool
routing, validation, the shared-working-tree git protocol, beads, and session
completion all live there. Global conventions are in
@/Users/aditya/.config/agents/AGENTS.md.

This file adds only what those two do not cover.

## Gotchas that cost a turn

- **The zero surface is narrower than it looks.** Search is literal-only, so
  regex alternation silently returns zero hits instead of erroring. `read` has no
  working line-range parameter, and read payloads truncate -- the visible capsule
  is not the whole file, so expand the result ref when you need exact bytes.
- **Writes must resolve under the session root.** `/tmp` paths are rejected;
  stage inside the repo, then move with `zero.token.shell`.
- **CI is deliberately off** in all four repos to avoid burning GitHub budget on
  every commit. An untracked `development-contract.yml` would switch it back on.

## Verification bar

A fix needs a test that fails without it. Mutation-test rather than assume:
revert the fix, watch the test go red, restore it. A test that passes either way
proves nothing, and this repo has already shipped one of those.
