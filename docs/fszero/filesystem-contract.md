# FSZero filesystem contract

Status: normative behavior summary. `zero-abi` defines the typed file-engine boundary, and `zero-fs` behavior tests enforce these invariants. FSZero has no separate operation catalog.

FSZero is ZeroStack's byte and filesystem domain library. ZeroKernel exposes that authority through `z.read`, `z.edit`, and `z.apply`. FSZero does not expose a model-facing tool catalog or operate as a separate product.

## Typed domain boundary

`zero-abi` defines file requests, outcomes, receipts, errors, cancellation, and budget context. `zero-fs` validates that boundary before filesystem work starts. Unknown fields, malformed inputs, stale authority, and unconfined paths fail typed.

## Roots and paths

File paths are UTF-8, workspace-relative strings. Absolute, drive-qualified, parent-traversing, and empty normalized paths are rejected. Existing targets and existing parents of new targets are canonicalized, then compared by path component against the canonical root. String-prefix containment is forbidden. Links that resolve outside the root are rejected.

Forward slash is the portable separator. Host component semantics, case behavior, Unicode normalization, reserved names, and path length remain host-defined. FSZero does not perform case folding or Unicode normalization.

## Reads and traversal

A full read is binary-safe and binds an immutable content ref to the exact bytes observed. A structured read may return a bounded projection plus an exact handle. Reading the handle recovers the original bytes.

Listings are deterministic and sorted by rendered relative path. Recursive and glob walks do not traverse symlinks. The portable glob subset is `*`, `?`, and `**`; classes and braces are rejected. A budget hit marks the result incomplete and cannot support an absence claim.

Line targets use `<path>#L<start>-L<end>`, with one-based inclusive bounds. Byte targets use `<path>#B<start>-<end>`, with zero-based half-open bounds.

## Metadata and links

A verified replacement writes a sibling temporary file and rename-replaces the directory entry. Replacing a symlink replaces the link entry rather than writing through it. Atomic replacement preserves exact bytes, Unix mode, and readable Unix extended attributes where the host supports them. Rollback restores journaled modification time and Unix mode. System-managed attributes that reject writes remain best-effort.

FSZero does not claim owner or group preservation, ACL preservation, creation-time preservation, Windows alternate streams, resource forks, sparse extents, or hard-link topology. Replacing one hard link does not mutate its siblings.

## Mutation and visibility

Every mutation validates its preimage before publication. A stale preimage fails closed. `z.edit` changes one file. `z.apply` validates and publishes one atomic effect set through the ZeroStack transaction boundary.

Same-directory rename provides old-or-new directory-entry visibility where the host supports atomic rename. Atomic visibility is not a power-loss durability claim. Durable stores sync their journal and content data according to their documented durability class.

Undo is a guarded compensating mutation. It does not erase history. A failed cell restores published receipts in reverse order. A rollback failure is a corrupt outcome, never success or cancellation.

## Cancellation and limits

Cancellation or deadline observed before publication rejects without changing the target. Work already published durably is never reported as cancelled. Blocking host calls are not universally preemptible.

Read, traversal, process, response, and recovery budgets remain binding. FIFOs, sockets, devices, and platform-specific reparse variants have no mutation guarantee unless a typed capability advertises one.

## Determinism and evidence

The same root, snapshot, request, and contract produce the same ordered domain result. Receipts bind the request, preimage, result, and durability state. A compact output is not byte authority; exact bytes and content-addressed handles remain authoritative.

## Platform boundaries

- macOS and Linux use the Unix path model. Mounted filesystems determine case and normalization behavior.
- Windows inputs remain workspace-relative. Drive-qualified, UNC, and rooted inputs are rejected even when the configured root uses them.
- Cross-platform behavior stops at the stated portable path, byte, ordering, and error contracts. Host-specific metadata remains outside the contract unless advertised explicitly.
