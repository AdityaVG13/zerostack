# Pack manifest canonical-v2 migration

GraphZero pack manifest schema 2 signs and hashes only the recursively key-sorted JSON bytes returned by `zero_abi::canonical_json`. Schema 1 used declaration-order JSON. The byte formats are intentionally not interchangeable.

## Compatibility policy

Normal reads, digesting, signing, and verification fail closed for schema 1 with `ManifestSchemaError::LegacyManifest`. The error directs operators to rebuild and explicitly re-sign as schema 2. There is no legacy verification API, no automatic encoder fallback, and no silent re-signing. Public keys and ed25519 verification remain unchanged.

## Schema-1 artifact inventory

The repository-relative audit found no checked-in `manifest.json`, detached signature, or signed golden artifact under `crates/graphzero-pack/`. Known schema-1 output sites were generated artifacts only:

- `crates/graphzero-pack/src/build.rs::build_pack_from_sources` and `build_fixture_pack` wrote `<output>/manifest.json`, with the signature embedded in `signature_hex`.
- `crates/graphzero-pack/tests/pack_contract.rs` generated signed manifests in temporary pack directories and installed copies in temporary stores.
- `crates/graphzero-pack/benches/pack_install.rs` generated the same fixture manifest in a temporary directory.

Migration action: discard these generated schema-1 manifests, rebuild from their source blobs with the schema-2 builder, and explicitly sign with the intended existing key. An installed schema-1 pack must be rebuilt and reinstalled; its old signature is never accepted or rewritten.
