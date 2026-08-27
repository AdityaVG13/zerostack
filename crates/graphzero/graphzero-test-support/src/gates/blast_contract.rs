//! Shared blast-radius contract checks on the canonical parse_ref fixture.

use crate::blast::blast_indexed_fixture;
use graphzero_engine::blast::{BlastRadiusCapsule, blast_radius};
use graphzero_store::Snapshot;

pub const PARSE_REF_INTENT: &str = "change signature of parse_ref";
pub const LOAD_CONFIG_INTENT: &str = "change signature of load_config";

pub fn blast_capsule(intent: &str) -> BlastRadiusCapsule {
    let fx = blast_indexed_fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open snapshot");
    blast_radius(&snapshot, intent, 800).expect("blast_radius")
}
