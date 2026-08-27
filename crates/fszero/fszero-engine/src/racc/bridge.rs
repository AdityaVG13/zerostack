//! Production bridge: RACC exact-authority types used by session/world paths.

use super::exact_snapshot::ExactSnapshot;
use super::safepoint::RawBaselineSafepoint;
use super::successor_map::{RefFate, SuccessorMap};
use std::collections::BTreeMap;
use std::path::Path;

/// Build an exact snapshot identity from path→bytes (world materialization).
pub fn snapshot_from_files(
    files: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<ExactSnapshot, super::exact_snapshot::SnapshotError> {
    ExactSnapshot::from_files(files)
}

/// Capture a project safepoint for a world/session baseline (filesystem only).
pub fn safepoint_for_snapshot(
    snap: &ExactSnapshot,
    journal_head: Option<&str>,
) -> RawBaselineSafepoint {
    RawBaselineSafepoint::capture(snap, None, journal_head, vec![], None)
}

/// Record path renames into a successor map (cache invalidation identity).
pub fn record_path_move(
    map: &mut SuccessorMap,
    from: &str,
    to: &str,
) -> Result<(), super::successor_map::SuccessorMapError> {
    map.record(RefFate::Moved {
        from: from.to_string(),
        to: to.to_string(),
    })
}

/// Materialize a directory tree into path→bytes for snapshotting (test/helper).
pub fn load_tree(root: &Path, rels: &[&str]) -> std::io::Result<BTreeMap<String, Vec<u8>>> {
    let mut out = BTreeMap::new();
    for rel in rels {
        let p = root.join(rel);
        if p.is_file() {
            out.insert((*rel).to_string(), std::fs::read(p)?);
        }
    }
    Ok(out)
}
