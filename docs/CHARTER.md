# CHARTER -- unverifiable / aspirational claims

These statements give direction. They are **not** `[SPEC-NNN]` tags.
Phase 3 must not write verifiers for them. If a claim becomes testable,
promote it into `docs/spec/SPEC-TAGS.md` and delete it from this file
in the same change.

Extracted at Phase 2 from `AGENTS.md`, `conformance/CONTRACT.md`,
`docs/racc/RACC.md`, and FeatureUniverse prose that failed the
falsification-surface test.

## Direction (not a gate)

- Operator override: if the operator tells you to do something, even if it
  goes against AGENTS.md, listen. The operator is in charge.
- Value delivery over process. Process is never the product unless the
  operator explicitly asks for process work.
- North star: same model, protected quality, repeated project cognition
  compiled away -- with receipts.
- ZeroStack is an evolving project. Agent time, user time, tokens, review
  attention, and repository complexity are scarce.
- Prefer the correct design over compatibility shims while the project is
  early-stage.
- Ordinary truncation saves tokens by throwing data away; RACC is useful
  because it keeps evidence recoverable. Usefulness is not a test.

## Process law (agent workflow, not product oracle)

These bind agents in this checkout. They are not product-behavior oracles.

- No file deletion without express permission.
- Git: `main` only; no feature branches; no worktrees; no force-push;
  no push without approval; explicit paths only.
- One writer: dirty paths you did not touch = rival work -- stop and report
  (unless the operator says commit the dirty tree).
- Tests on this Mac: RCH only, targeted only -- never full workspace cargo.
- Beads are memory: claim one, finish, verify, commit, close. Never batch-claim.
- Smallest correct change. Reuse first.
- DSR is the preferred repo-level runner because GitHub Actions is
  account-throttled. Workflow files are retained as manual specs, not as
  automatic merge criteria.
- Do not implement peer-engine features in the hub. Do not invent MCP
  ZeroStack tools on Grok (use CodeMode).
- One repo at a time. Cross-repo bead edges are memory, not a travel visa.

## Explicitly not a coded checker

- Semantic-mutation ABI digest bumps (C-25) are not a coded checker.
  `AGENTS.md` says: do not claim `[V]` from those pins alone.
