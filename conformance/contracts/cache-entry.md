# cache-entry

Normative contract for `crates/zero-abi/src/cache_entry.rs`.

A cache hit can only be built from a key carrying a completeness witness.
Constructors and deserializers validate that witness before accepting an
entry. The unsound direction (under-invalidation) fails closed.

Schema id: `cache-entry`. Roots are non-empty content-addressed strings.
The canonical key JSON is hashed with SHA-256 (`sha256_hex`) and used as the
cache key.
