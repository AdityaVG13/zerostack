# Cache Entry v1

The shared zero-abi::cache_entry module is the ABI owner for cache entries.
It lives in zero-abi, rather than zero-ledger, because both GraphZero witness
caches and FSZero memoization consume the same wire schema; the ledger records
accounting, but does not own cache validity.

## Schema

The wire discriminator is schema: "cache-entry/v1".

| Object | Field | Meaning |
| --- | --- | --- |
| key | operator.id | Stable operator identifier |
| key | operator.version | Locked operator implementation/version |
| key | canonical_parameters | Canonical JSON parameters; object keys are sorted for hashing |
| key | minimum_dependency_roots | The minimum exact content roots read by the operation. This is a set, not a repository-root hash. |
| key | environment_roots | Exact environment/input roots that affect the result |
| key | toolchain_roots | Exact compiler/parser/indexer/toolchain roots that affect the result |
| key | completeness_witness | Durable proof root plus the roots checked for completeness; required for every entry |
| key | scope_roots | Anti-dependency roots for a negative entry; empty for a positive hit |
| value.kind=hit | output_root | Content root of the cached output |
| value.kind=hit | verifier_receipt | Optional independent verifier receipt root and verifier identity |
| value.kind=no_matches | (unit value) | A certified no-matches answer; its scope roots are in the key |

All roots are non-empty opaque content-addressed root strings. Root sets are
sorted and deduplicated before serialization. Canonical key hashing is
SHA-256(canonical_json(key)), where canonical JSON sorts object keys
recursively. This makes parameter-map order and input root order irrelevant,
while preserving array order as part of parameter meaning.

## Key derivation

1. Lock the operator id and version before execution.
2. Canonicalize the parameter value as JSON.
3. Record the minimum exact dependency set actually required for the result.
   Do not substitute a whole-repository root or another coarse snapshot key:
   that causes avoidable over-invalidation and defeats reuse.
4. Record exact environment and toolchain roots separately.
5. Produce a completeness witness whose proof root durably identifies the check
   and whose checked roots cover the dependency cone. A key without this witness
   is not a valid cache key and cannot be deserialized as an entry.
6. For a no-matches answer, add every scope/anti-dependency root that was
   searched to scope_roots, and ensure the witness covers each one.

The key hash is the lookup identity. A hit is reusable only when every key root
still resolves to the same content and the operator/version and parameters
match byte-for-byte under the canonical form.

## Invalidation semantics

A dependency-cone change invalidates an entry when it changes any exact minimum
dependency root, environment root, toolchain root, or completeness-witness
input root. For a negative entry, a change to any scope_root also invalidates
it: new content in that searched scope can turn no matches into a match.
The cache must re-run the operator when a root cannot be resolved or the witness
cannot be verified. Never silently treat missing evidence as unchanged.

The dependency set is deliberately minimum and exact. A repository-root key is
not a correctness requirement, but it is an over-invalidation quality failure:
it invalidates unrelated edits and drives the fresh-work fraction upward.

## Negative entries

CacheEntryV1::negative stores value.kind = no_matches. It requires a
non-empty scope_roots set in the key. Those roots are anti-dependencies, not
an output root: they describe the complete search scopes whose current content
supports the empty answer. They are included in the same canonical key hash and
must be covered by the completeness witness. This makes empty answers reusable
and precisely invalidated rather than treating them as uncachable misses.

## Safety asymmetry

Under-invalidation, meaning returning a stale result after a relevant input
changed, is forbidden and unsound. The Rust API makes this direction hard:
entries have private fields, positive/negative constructors validate key roots,
and completeness_witness is required by the key wire shape and verified during
deserialization. Missing or invalid witness data therefore fails closed.

Over-invalidation, meaning rejecting a still-valid entry because a coarse or
unrelated root changed, is a quality bug but never unsound. Implementations may
conservatively invalidate while repairing dependency extraction, but must not
replace the exact dependency set with a repository-wide hash in the v1 schema.
