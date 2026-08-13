# Public surface inventory

Status: proposal only. Inventory source: `git ls-files` and `git ls-files --others --exclude-standard`.
No path was deleted, untracked, or added to the active `.gitignore`.

## Intended public authority

| Surface | Current tracked files | Current bytes | Decision |
|---|---:|---:|---|
| `crates/` | 130 | 2,635,241 | Keep. Production Rust authority. |
| `conformance/` | 49 | 625,945 | Keep pending per-artifact review. Cross-engine contracts, schemas, models, and proof receipts. |
| `docs/` | 11 | 117,495 | Keep user and architecture docs. Review the five legacy `docs/racc/` files after `docs/racc-r.md` exists. |
| `scripts/` | (install CLIs removed) | — | Policy/portability checks only. `zs` and installers parked during corpus shift. |
| `README.md`, `INSTALL-FOR-AGENTS.md` | 2 | — | README is the human entry. INSTALL-FOR-AGENTS is a parked stub, not an installer. |
| `Cargo.toml` | 1 | 616 | Keep as the workspace manifest. |
| `.github/` | 1 | 3,486 | Keep reviewed CI. A second workflow is untracked agent work and is not classified here. |
| `formal/lean/` | untracked work | 18,828 | Intended public formal authority after its bead passes the pinned build and trust gates. |
| `docs/papers/` | untracked work | 19,182 | Intended public six-paper scaffold after its claim-language gate passes. |

The tracked baseline contains 212 files. Counts and byte sizes use the working-tree copies at inventory time.

## Tracked private or generated candidates

These paths are already tracked. Ignore rules alone will not remove them from the public repository.
Untracking or deletion needs separate operator approval.

| Candidate | Files | Bytes | Reason |
|---|---:|---:|---|
| `.beads/` | 7 | 1,926,131 | Internal issue memory and backups. |
| `AGENTS.md` | 1 | 7,556 | Private program law; repository policy already says not to publish it. |
| `CLAUDE.md` | 1 | 120 | Private harness routing. |
| `docs/.DS_Store` | 1 | 6,148 | OS metadata. |

The `.beads/` total includes `issues.jsonl` (1,132,299 bytes) and
`issues.jsonl.bak-1f3t-salvage` (789,073 bytes). This inventory does not authorize
removing either file.

## Untracked local-state candidates

The following untracked groups are local state or scratch, not proposed public artifacts:

| Pattern | Files | Bytes |
|---|---:|---:|
| `.ee/` | 331 | 9,897,196 |
| `.research-*` | 6 | 2,092,200 |
| `.beads/` local state | 4 | 1,098,173 |
| `cmd-group-*` | 14 | 61,338 |
| `pw-*` | 9 | 57,861 |
| root `.g*.txt` / `.r*.txt` scratch | 6 | 12,526 |
| `.papercuts.jsonl` | 1 | 12,412 |
| `AGENTS.md.pre-*.bak` | 1 | 7,310 |
| `.mcp.json` | 1 | 245 |

Untracked conformance receipts, the RACC native workflow, `formal/lean/`, and
`docs/papers/` are active agent work. They are excluded from the private-state
proposal until their owning beads decide their public status.

## Operator decisions required before cleanup

1. Decide whether `docs/adr/` stays public, becomes private, or gets public summaries. It governs engine-facing boundaries but is currently ignored and untracked.
2. Approve or reject untracking the four tracked private/generated candidates above.
3. Review whether every conformance proof receipt is a release artifact before narrowing the public allowlist.
4. Merge or supersede legacy `docs/racc/` only after its public replacement carries the same authority.
