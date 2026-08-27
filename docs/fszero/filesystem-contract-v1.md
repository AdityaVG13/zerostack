# FSZero filesystem contract v1

Status: normative version 1.0.4. Beads: `fszero-ai-filesystem-excellence-jqf.8`, `fszero-ncib.1`, `fszero-bhc1`.

The canonical, machine-readable contract is [`contracts/filesystem-v1.json`](../contracts/filesystem-v1.json). This document is a reading guide, not a second source of semantics. The Rust API parses that checked-in JSON once and exposes the same value through:

- `filesystem_contract_descriptor()` for embedded callers;
- `FSZeroSession::root_report()["filesystem_contract"]` for doctor/CLI;
- recovery key `filesystem_contract`, expandable through MCP and CodeMode;
- the public constants `FILESYSTEM_CONTRACT_NAME`, `_MAJOR`, `_MINOR`, and `_VERSION`.

## Operation ABI (typed domain surface)

Bead `fszero-ncib.1` adds a **canonical operation ABI** in `src/core/operation_abi.rs` (`OPERATION_REGISTRY`) plus full **input/output JSON Schema ownership** in [`contracts/operation-abi-schemas-v1.json`](../contracts/operation-abi-schemas-v1.json) (loaded by `src/core/operation_schemas.rs`).

- Domain ops, MCP tools, CodeMode tools, and CodeMode methods each have complete schema structure (properties, types, `required`, constraints, optional `output`).
- Live MCP/CodeMode tool catalogs are **materialized** from that document (`mcp_tools()` / `codemode_tools()`); exact structural parity rejects missing/extra properties, type changes, requiredness, constraint, and output-shape drift.
- `operation_abi_digest()` hashes the registry **and** the full schema document; `schemas_digest` is published alongside it.
- `golden_vectors.abi_domain` covers success, typed failure, ref recovery, mutation reject, deadline, and cancellation outcomes.
- Memory, history, undo, and world are first-class contract operations with surface aliases (`fszero-ip16.1`, `fszero-ip16.7`, `fszero-ivee.4` absorbed).

## Compatibility

The protocol boundary is `fszero-filesystem` major 1. Missing, malformed, differently named, or unknown major versions fail with `incompatible_contract` before filesystem work. Higher minor versions and unknown additive fields are accepted. Existing fields cannot change meaning inside major 1.

Version 1.0.0 freezes current behavior. Its only runtime wire change is the additive `filesystem_contract` root-report/recovery value. It changes no mutations or on-disk format. Rolling back to an older binary merely removes that field; stores, journals, refs, and workspaces need no migration.

## Normative guarantees

### Roots and paths

Operation paths are UTF-8, workspace-relative strings. Absolute, drive-qualified, parent-traversing, and empty normalized paths are rejected. Existing targets and existing parents of new targets are canonicalized, then compared by path component against the canonical root. String-prefix containment is forbidden. Links resolving outside the root are rejected.

Forward slash is the portable separator. Host component semantics, case behavior, Unicode normalization, reserved names, and path length remain host-defined. FSZero performs no case folding or Unicode normalization.

### Reads and traversal

Full reads are binary-safe and return an immutable content ref for the exact bytes observed. `path#Bstart-end` live ranges are zero-based and end-exclusive, clamp to the current length, reject reversed endpoints, and cap one range at 8 MiB.

Listings are sorted by rendered relative path. CodeMode compound `list` paths are workspace-relative to the active root; use `path: "."` for the root instead of an absolute path. Recursive/glob walks do not traverse or return symlinks. The portable glob subset is `*`, `?`, and `**`; classes and braces are rejected. A budget hit marks an incomplete result.

### Links and metadata

A verified edit/write uses a sibling temporary file and rename-replaces the directory entry. Replacing a symlink replaces that link entry rather than writing through it. Atomic replacement preserves exact bytes, Unix mode, and readable Unix extended attributes. Rollback restores journaled mtime and Unix mode. System-managed attributes that reject writes are best-effort.

Version 1 does not promise owner/group, ACL, creation-time, Windows alternate-stream, resource-fork, sparse-extent, or hard-link-topology preservation. Atomic replacement of one hard link does not mutate sibling links.

### Mutation, visibility, and durability

Single-file rename publication provides old-or-new directory-entry visibility where the host supplies atomic same-directory rename. Atomic visibility is not power-loss durability: ordinary edit/write does not promise both file fsync and parent-directory fsync.

Verified edit, undo, and world commit reject stale preimages. World preview is overlay-only; drop leaves the base unchanged; commit performs preimage-guarded three-way application and rejects overlapping conflicts.

CodeMode transaction rollback is compensating recovery. Version 1 does not claim multi-file atomic visibility to unrelated processes or full multi-process serializability. History is retained mutation evidence; undo creates another guarded mutation rather than erasing history.

### Cancellation and limits

Cancellation or deadline observed before publication rejects without changing the target. Already published durable work is never reported as cancelled. Blocking host calls are not universally preemptible.

The explicit full-read ceiling is currently absent; callers should range large files. Traversal, range, CodeMode, and output budgets remain binding. FIFOs, sockets, devices, and reparse variants have no mutation guarantee unless a later capability advertises one.

### Platforms

- macOS: Unix path model; mounted volume determines case and normalization. Unix modes and readable xattrs are supported. Resource-fork, ACL, and normalization equivalence are not claimed.
- Linux: Unix paths exposed through UTF-8 string APIs. Invalid-UTF-8 names are not portable. Unix modes and readable xattrs are supported. ACL/security-label restoration is not claimed.
- Windows: Windows components and separators, but operation inputs remain workspace-relative. Drive-qualified, UNC, and rooted inputs are rejected even if the configured root uses them. Reparse-point, long-path, sharing-retry, ACL, and alternate-stream conformance awaits retained Windows evidence.


## Surface product decisions (fyx0 / bhc1)

Alias honesty: every public MCP tool name, CodeMode method path, and CLI opcode is dual-written into `OPERATION_REGISTRY` and `contracts/filesystem-v1.json` `aliases`. Live MCP/CodeMode catalogs are compared to that map. Registry CLI opcodes must also appear under `aliases.cli` (no one-sided registry letters).

### CLI opcode `M` — PRESENT (aligned)

| Surface | Binding |
|---|---|
| CLI | `M` → `fs.memory` (durable `mem://` volume) |
| MCP | `fszero.memory_{put,get,ls,delete,rename}` → `fs.memory` |
| CodeMode | `fs.memory.{put,get,ls,delete,rename}` → `fs.memory` |

`M` was already live on the session `OpCode` table and registry `cli_opcodes`; 1.0.4 dual-writes it into the contract CLI map so doctor/parity tests cannot drift.

### `fs.listMany` single-letter CLI — DROPPED

`fs.listMany` has **no** single-char CLI opcode (`cli_opcodes` empty). It is not missing dual-write; the letter space is reserved for primary interactive ops.

| Surface | Binding |
|---|---|
| MCP | `fszero.list_many` |
| CodeMode | `fs.listMany` |
| CLI | packaging **`batch`** subcommand only — JSON envelope `{"operation":"fs.listMany","args":{...}}` (same path as other `*Many` vectorized ops) |

### Compound MCP policy

Canonical op: `fs.compound`.

| Surface | Binding |
|---|---|
| CodeMode | first-class `fs.compound` (and named compounds / recipes) |
| CLI | opcode `C` |
| MCP | **no** `fszero.compound` tool |

MCP multi-step / loop work belongs on CodeMode. MCP-only installs may use the surface-dispatch escape hatch `fszero.exec` with a CLI letter (including `C` for compound). That tool maps to `surface_dispatch`, not a single domain op.

### Ghost-alias rules

1. **Contract dual-write.** Every registry MCP alias, CodeMode alias, and CLI opcode must appear under the matching `aliases.*` map (except tools that intentionally target `surface_dispatch`).
2. **Surface-dispatch only.** Aliases that are not a single canonical op are listed in `SURFACE_DISPATCH_ALIASES` (`mcp`/`fszero.exec`, `embedded`/`FSZeroSession.execute`) with contract target `surface_dispatch`. They must not claim a fake domain op id.
3. **No ghosts.** Live MCP/CodeMode catalogs equal the contract maps. `resolve_alias` must match contract targets. Empty `cli_opcodes` on vectorized `*Many` ops is intentional, not a ghost.
4. **No invented MCP compounds.** Do not add `fszero.compound` or other compound-shaped MCP tools without a product decision that dual-writes registry + schemas + contract; default remains CodeMode-owned.

## Stable errors and operation coverage

`error_classes` in the canonical JSON defines the stable taxonomy. Every operation in `operations` names its controlling clauses and allowed stable errors. `aliases` maps CLI opcodes, MCP tool names, CodeMode methods, and embedded entry points back to those operations. The focused parity test compares the live MCP and CodeMode catalogs to this map, preventing a new public operation from bypassing the contract.

## Golden vectors

`golden_vectors` covers valid/invalid relative paths, Unix absolute paths, Windows drive/UNC inputs, Unicode preservation, live byte ranges, stale verified edits, overlapping world commits, cancellation before publication, separator divergence, case behavior, and invalid-UTF-8 Linux names. Runtime tests additionally prove root-report/recovery parity and stale-edit rejection without changing the target.
