# Shared-CAS GC metadata contract v1

Bead: `tokenzero-9ap`

This document freezes the engine-neutral metadata contract identified by `zerostack.cas-gc.legacy`. TokenZero owns the contract; TokenZero, FSZero, and GraphZero are peer producers and consumers. RFC 2119 terms are normative.

## 1. Scope and safety invariant

The shared CAS stores immutable bytes at `<store-root>/blobs/sha256/<2hex>/<64hex>`. Engine-private data remains under `<store-root>/tokenzero/`, `<store-root>/fszero/`, and `<store-root>/graphzero/`. GC coordination metadata lives only under `<store-root>/gc/` and MUST NOT depend on any engine-private database, cache, or reference type.

A collector MUST retain an object whenever metadata is missing, unreadable, unparseable, internally inconsistent, unsupported, newer than the collector, or otherwise uncertain. It MUST NOT delete any CAS object unless every applicable v1 record was read successfully and the object has no current root, pin, active lease, or grace-protected stale lease. An unknown/newer version or corrupt metadata file makes the entire store unsafe for deletion for that run: every candidate verdict MUST be `retain-uncertain`.

## 2. Version and JSON rules

Every record has `schema_version: "zerostack.cas-gc.legacy"` and a record-specific `record_type`. JSON is UTF-8. Producers MUST emit schema-valid records. Consumers MUST reject duplicate object keys and non-finite numbers. v1 consumers MUST treat any other schema version as unsupported, including lexically similar newer versions.

Schemas use JSON Schema 2020-12 and are frozen under `schemas/shared-cas-gc/v1/`. Golden fixtures are normative examples.

## 3. Namespace and path grammar

`engine` is exactly `tokenzero`, `fszero`, or `graphzero`. `project_id` is 64 lowercase hexadecimal characters: SHA-256 of a producer's stable canonical project identity. The identity derivation is producer-owned, but it MUST distinguish projects that may share a store and MUST remain stable across operations. Path components MUST be validated against the schema before joining; separators, `.`, and `..` are impossible under this grammar.

Published records:

- root: `<store-root>/gc/roots/<engine>/<project_id>/current.json`
- pin: `<store-root>/gc/pins/<engine>/<project_id>/<pin_id>.json`
- lease: `<store-root>/gc/leases/<engine>/<project_id>/<operation_id>.json`
- optional dry-run report: `<store-root>/gc/reports/<run_id>.json`

The namespace fields inside each record MUST equal its path components. Roots, pins, and leases from different engine/project namespaces are independent even when they name the same blob hash. A collector computes the union; it MUST NOT let one namespace overwrite, shadow, or release another.

Blob identifiers in metadata are full lowercase SHA-256 digests, naming `blobs/sha256/<first-two-hex>/<digest>`. URI schemes and fragments are not stored.

## 4. Reachability snapshots and atomic publication

A reachability snapshot is the complete current root set for exactly one engine/project namespace at a monotonically increasing `epoch`. Producers MUST publish it atomically on the same filesystem:

1. write a uniquely named temporary sibling of `current.json` (recommended `.current.<nonce>.tmp`);
2. flush the complete file and, where durability is required, fsync the file;
3. rename the temporary sibling over `current.json` with one atomic replace operation;
4. where durability is required, fsync the containing directory.

Producers MUST NEVER modify `current.json` in place. Collectors MUST read only `current.json`; temporary siblings are not snapshots. If publication is observed in an uncertain state (I/O error, malformed current file, or inability to establish atomic replacement), the whole-store retain-on-uncertainty rule applies. A lower epoch replacing a previously observed higher epoch is inconsistent and therefore unsafe.

## 5. Pins

A pin protects one blob independently of reachability snapshots. A pin is active from `created_at` until `expires_at`, if present; absence of `expires_at` means no automatic expiry. Deleting a pin record is its explicit release. An expired pin contributes no root only when its record was parsed and evaluated with a trustworthy clock; clock uncertainty retains.

## 6. Leases, epochs, and grace

A lease protects `blob_hashes` used by one active operation. It carries a producer-monotonic positive `epoch` plus owner `pid`, `host`, and `started_at`, and an `expires_at`. A producer MUST atomically replace its lease record when renewing it and MUST increase the epoch when reusing an operation ID for a new operation. A collector MUST retain when epochs regress or conflict.

Let `G = grace_seconds` and `E = expires_at`:

- at or before `E`, the lease is active and its blobs MUST be retained;
- after `E` but before `E + G`, the lease is stale-inside-grace and its blobs MUST be retained, even when the owner is positively dead;
- at or after `E + G`, the lease may cease protecting blobs only if the owner is positively confirmed dead locally or by an authoritative host-specific mechanism, or a valid higher epoch for the same namespace/operation supersedes it;
- an unreachable host, permission failure, clock uncertainty, unsupported owner check, or any other indeterminate liveness result MUST produce `retain-uncertain`.

`grace_seconds` is a per-record minimum and MUST be at least 60. Consumers MAY apply a larger configured grace, never a smaller one. PID reuse alone is not positive liveness evidence; host and operation start identity must also agree.

## 7. Version negotiation and corruption

Consumers support an explicit set of exact schema versions; v1 defines no forward-compatible extension mode. Discovery includes every regular entry under `gc/roots`, `gc/pins`, and `gc/leases`. An unknown file, unexpected path, unknown/newer version, schema violation, duplicate key, corrupt JSON, or read error MUST stop deletion for the entire store run. The collector MAY continue to produce a dry-run report, but all candidate verdicts MUST be `retain-uncertain` with an uncertainty reason.

## 8. Dry-run explanation reports

A dry run emits one report conforming to `dry-run-report.schema.json`. It contains exactly one explanation for each candidate object. `verdict` is `retain`, `collect`, or `retain-uncertain`. `reason_codes` is non-empty and machine-readable; `evidence` names applicable metadata paths. `collect` is permitted only with `no-live-reference`; uncertainty reasons require `retain-uncertain`. Reports explain decisions but are never themselves GC roots.

## 9. Conformance and sibling engines

A conforming producer needs only its engine name, stable project identity, full CAS hashes, operation/pin identifiers, wall-clock timestamps, and lease owner identity. No field requires TokenZero-private state. FSZero bead `fszero-c6q.9` and GraphZero bead `graphzero-zeroref-v1-shared-cas-1ghi.9` can implement this contract verbatim using the documented `<store-root>` layout and their own root enumeration. They MUST NOT read `<store-root>/tokenzero/` or encode engine-private refs in these records.

A conforming collector MUST validate the namespace/path equality, exact version, schemas, atomic-current convention, epoch monotonicity when prior state is available, lease grace/liveness rules, and whole-store retain-on-uncertainty invariant before deletion.

## 10. Fixture organization

The repository keeps the cqr.2 ZeroRef vectors as `tests/zeroref-v1-golden-vectors.json` and existing normative contracts under `docs/`, but `.gitignore` freezes `/docs/*` against new untracked documents. Therefore this new multi-document normative contract is a self-contained, versioned bundle at `schemas/shared-cas-gc/v1/`: this `SPEC.md`, the JSON Schemas, and fixtures are colocated so sibling engines can vendor or execute the directory unchanged without changing an existing index or ignore rule. Version naming follows the existing dotted `tokenzero.migration.v2` style while using the engine-neutral `zerostack` namespace.
