//! P5.5 dependency shard packs: signed manifests, reproducible build, dedup install.

mod build;
mod install;
mod manifest;
mod query;
mod semantic_index;
mod sign;

pub use build::{
    build_fixture_pack, build_pack_from_sources, reproducible_manifest_digest,
    verify_pack_artifacts,
};
pub use install::{InstallReport, install_pack, list_installed, uninstall_pack};
pub use manifest::{
    LEGACY_MANIFEST_SCHEMA_VERSION, MANIFEST_SCHEMA_VERSION, ManifestSchemaError, PackManifest,
    PackProvenance, PackSemanticSidecarEntry, PackShardEntry, PackSignKey,
};
pub use query::{pack_tier_a_coverage, query_pack_symbol, query_pack_symbol_in_version};
pub use semantic_index::{
    attach_semantic_sidecars, build_semantic_sidecars_for_pack, golden_hash_for_installed_sidecar,
    install_semantic_sidecars,
};
pub use sign::{sign_manifest, verify_manifest_signature};
