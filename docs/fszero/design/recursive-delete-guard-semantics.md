# Recursive delete permission semantics -- engine note

Bead: `fszero-recursive-delete-guard-semantics-4t0p`
Date: 2026-08-07
Lens: DeepAgents 0.7 recursive-delete hardening (write class + bulk path-overlap deny + not-found + symlink jail)

## Questions

1. Is recursive workspace delete exposed anywhere (compound mutate, world ops, MCP/CLI)?
2. Do guard checks use subtree path-overlap, or only exact paths?
3. Are symlink-resolved escapes outside the session root filtered?

## Findings

### 1. No recursive / bulk workspace delete API

| Surface | Delete behavior |
| --- | --- |
| `fs.compound('mutate'\|'edit', {delete:true})` | **Rejected** with corrective error: compound has no delete API (`src/codemode/connector.rs`). |
| `fs.write` / `fs.edit` / compound mutate | Write or preimage-replace only; never `remove_dir_all` on workspace trees. |
| World `new`/`edit`/`commit`/`drop` | Per-file publish/rollback; may `remove_file` for created paths or empty-pre rollback -- **not** recursive tree delete. |
| MCP `fszero.*` | Same kernel ops; no recursive-delete tool in catalog. |
| CodeMode / recipes | `fs.memory.delete` only for durable `mem://` keys (below). |

Regression pin already present: `src/codemode/js.rs` asserts compound delete is rejected (`does not support delete`).

### 2. Memory delete is single-key, exact path

`delete_memory` / opcode `M` / `fs.memory.delete` / MCP `fszero.memory_delete`:

- Normalizes one logical path under `mem://` (`mem_key`); rejects `..` components.
- Deletes **one** store key + `memory_paths` index row in one exec txn.
- Missing key returns `memory miss: …` (not-found), not a silent success.
- **No prefix / recursive mem-tree delete.** Deleting `a` does not remove `a/one.md`.

Mutability: `fs.memory.delete` is classified **mutating** for plan transactions (`call_is_mutating` in `src/codemode/transaction.rs`). ABI row `fs.memory` is `Mutability::Mixed` (read get/ls + write put/delete/rename).

### 3. Path-overlap deny lists: N/A at engine (no bulk delete)

DeepAgents needed bulk path-overlap because a recursive delete tool expands to many descendants. FSZero does **not** expose that tool class, so there is no engine-side multi-path deny expander to mis-implement.

What exists instead:

- **Exact-path root jail** for workspace I/O: `resolve_existing_path` / `revalidate_path_under_root` / `canonical_path_within_root` (`src/core/path.rs`) -- canonicalized target must stay under session root.
- **Symlink escape tests** on mutate/edit paths: `fused_snap_scope_rejects_symlink_escape`, `windowed_edit_rejects_symlink_escape_with_typed_root_failure` (`src/codemode/connector.rs`).
- **Write-side jail** never uses the read-only scratch allowlist (`FSZERO_SCRATCH_DIR`); writes stay root-jailed.
- **Harness DCG** remains the outer bulk-deny layer for agent tool plans; this audit is engine defense-in-depth only.

### 4. Residual risks (no product gap this bead)

| Residual | Severity | Notes |
| --- | --- | --- |
| Multi-file world commit can touch many paths | medium (by design) | Each path is individually staged/validated; not recursive delete of a subtree. Covered by world contract + crash-intent beads (`fszero-k4ur.*`). |
| Empty `fs.write` can blank a file | low | Content replace, not unlink; journaled. |
| Undo of created write deletes one file | low | Exact path via `validate_rollback_path`; expected undo semantics. |
| Future recursive-delete API | process | If ever added: must be write-classified, expand deny rules by **path-overlap** (descendants), not-found on missing roots, and re-run symlink jail on every expanded path. |

## Verdict

**No code change required for this bead.** Engine layer already:

1. Refuses recursive/compound delete of workspace trees.
2. Treats the only delete surface (`mem://`) as single-key write with not-found.
3. Root-jails and symlink-rejects workspace mutation targets.

Deliverable for acceptance: this audit note. Follow-up only if a product decision adds a recursive delete surface -- file a new bead with the DeepAgents checklist as acceptance criteria; do not bolt overlap deny onto non-existent bulk delete.

## Checker (local, no full suite)

```bash
rg -n 'does not support delete|delete_memory|remove_dir_all' src/codemode/connector.rs src/core/memory.rs src/core/compound_ops.rs
rg -n 'symlink_escape|canonical_path_within_root' src/codemode/connector.rs src/core/path.rs
# targeted (RCH), optional:
# rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo test -p fs-zero --lib compound_mutate -- --test-threads=1
```
