# Shared content-addressed store

ZeroStack stores immutable recovery objects in the canonical CAS implemented by `crates/zerostack/zero-store`. FSZero supplies byte authority and may publish verified objects into that store. GraphZero and TokenZero access shared objects only through ZeroStack-owned contracts; engines never import one another.

## Layout

Objects are addressed by the full lowercase SHA-256 digest of their bytes. The store uses a deterministic fan-out path beneath the resolved ZeroStack state root. Paths are an implementation detail; `z://blob/<digest>` is the portable identity.

A valid object has these properties:

- its file name and directory placement agree with the full digest;
- the stored bytes hash to that digest;
- the object is immutable after publication;
- a reader verifies identity before returning bytes;
- a fragment is applied only after full-object verification.

## Publication

Writers create and sync a temporary object, then publish it with an atomic same-filesystem rename. Publishing bytes already present under the same digest is idempotent. Conflicting bytes under an existing digest are corruption and fail closed.

The CAS is not a transaction log. Mutable indexes, receipts, journals, reservations, and session state remain outside the immutable object namespace.

## Resolution

ZeroKernel resolves a portable ref through its configured store roots and policy. Resolution distinguishes:

- `missing`: no allowed store contains the object;
- `policy_denied`: a store exists but policy forbids access;
- `io`: store access failed;
- `digest_mismatch`: bytes do not match the requested identity.

String retagging or a matching filename never proves identity. The full bytes must hash correctly.

## Concurrency and recovery

Concurrent publication of identical bytes converges on one object. Readers never consume a temporary file. Garbage collection treats live refs, receipts, snapshots, and retained recovery state as roots. It must not remove an object reachable from any authoritative root.

A crash may leave an unreferenced temporary file. Recovery may quarantine or remove that temporary artifact after proving it is not authoritative. Published objects remain immutable.

## Privacy and scope

Store roots are explicit. ZeroStack does not scan unrelated user directories or upload objects. Sharing a CAS root shares immutable content by digest, not project metadata, graph state, token ledgers, or credentials.
