//! Indexed-vs-disk delta since last `graphzero index` (snapshot baseline).

use std::collections::{BTreeMap, BTreeSet};

use graphzero_store::Snapshot;
use graphzero_store::store::indexer;
use graphzero_store::store::query::{StalenessVerdict, blob_staleness_verdict};

use super::QuerySurfaceRouter;
use super::helpers::{empty_capsule, outline_items_for_path};
use super::skeleton::{byte_span_to_lines, format_outline_skeleton};
use super::types::{
    DeltaPayload, OutlineItem, QuerySurfaceError, QuerySurfaceRequest, QuerySurfaceResponse,
};

const MAX_SKELETON_FILES: usize = 10;

pub struct DeltaComputation {
    pub since: String,
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged_count: usize,
}

pub fn compute_repo_delta(snapshot: &Snapshot) -> Result<DeltaComputation, QuerySurfaceError> {
    let repo = snapshot
        .repo_root
        .as_ref()
        .ok_or(QuerySurfaceError::MissingArgument("repo"))?;
    let indexed: BTreeMap<String, (String, graphzero_store::store::query::PathRecord)> = snapshot
        .path_records()
        .map(|(h, r)| (r.path.clone(), (h.to_hex(), r.clone())))
        .collect();
    let disk_map =
        indexer::worktree_content_map(repo).map_err(|_| QuerySurfaceError::EvidenceMissing)?;

    let mut changed = Vec::new();
    let mut removed = Vec::new();
    let mut unchanged_count = 0usize;

    for (rel, (hash_hex, rec)) in &indexed {
        if !repo.join(rel).is_file() {
            removed.push(rel.clone());
            continue;
        }
        match blob_staleness_verdict(repo, rel, hash_hex, rec) {
            StalenessVerdict::Fresh | StalenessVerdict::StatOnlyChange => unchanged_count += 1,
            StalenessVerdict::Stale | StalenessVerdict::Unreadable => changed.push(rel.clone()),
            StalenessVerdict::Missing => removed.push(rel.clone()),
        }
    }

    let indexed_paths: BTreeSet<String> = indexed.keys().cloned().collect();
    // The index dedupes blobs by content hash (one PathRecord per hash), so a
    // path can be absent from the index while its exact bytes are already
    // known (e.g. two identical fixture files). "Added" means NOVEL content:
    // unknown path AND unknown hash. Known-hash aliases are counted as
    // unchanged — the model has comprehended those bytes.
    let indexed_hashes: BTreeSet<&str> = indexed
        .values()
        .map(|(hash_hex, _)| hash_hex.as_str())
        .collect();
    let mut alias_count = 0usize;
    let mut added: Vec<String> = disk_map
        .iter()
        .filter(|(path, hash)| {
            if indexed_paths.contains(*path) {
                return false;
            }
            if indexed_hashes.contains(hash.as_str()) {
                alias_count += 1;
                return false;
            }
            true
        })
        .map(|(path, _)| path.clone())
        .collect();
    unchanged_count += alias_count;
    added.sort();
    changed.sort();
    removed.sort();

    Ok(DeltaComputation {
        since: snapshot.entry.snapshot_id.to_string(),
        changed,
        added,
        removed,
        unchanged_count,
    })
}

impl QuerySurfaceRouter {
    pub(super) fn delta(
        snapshot: &Snapshot,
        _req: &QuerySurfaceRequest,
        _budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let comp = compute_repo_delta(snapshot)?;
        let skeletons = skeletons_for_changed(snapshot, &comp.changed)?;
        let capsule = graphzero_store::store::query::QueryEngine::warm(snapshot, "delta", 1)
            .unwrap_or_else(|_| empty_capsule("delta", snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "delta".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            delta: Some(DeltaPayload {
                since: comp.since.clone(),
                changed: comp.changed.clone(),
                added: comp.added.clone(),
                removed: comp.removed.clone(),
                unchanged_count: comp.unchanged_count,
            }),
            skeletons,
            ..Default::default()
        })
    }
}

fn skeletons_for_changed(
    snapshot: &Snapshot,
    changed: &[String],
) -> Result<Vec<String>, QuerySurfaceError> {
    changed
        .iter()
        .take(MAX_SKELETON_FILES)
        .map(|rel| skeleton_line_for_path(snapshot, rel))
        .collect()
}

fn skeleton_line_for_path(snapshot: &Snapshot, rel: &str) -> Result<String, QuerySurfaceError> {
    let repo = snapshot
        .repo_root
        .as_ref()
        .ok_or(QuerySurfaceError::MissingArgument("repo"))?;
    let stale_content = graphzero_store::store::query::path_record_for_rel(snapshot, rel)
        .map(|(hash_hex, rec)| {
            matches!(
                blob_staleness_verdict(repo, rel, &hash_hex, rec),
                StalenessVerdict::Stale | StalenessVerdict::Unreadable
            )
        })
        .unwrap_or(true);

    let outline = if stale_content {
        outline_from_disk(snapshot, rel)?
    } else {
        outline_from_index(snapshot, rel)?
    };

    let mut skeleton = format_outline_skeleton(rel, &outline);
    if stale_content && outline.is_empty() {
        skeleton = format!("stale:{skeleton}");
    }
    Ok(skeleton)
}

fn outline_from_index(
    snapshot: &Snapshot,
    rel: &str,
) -> Result<Vec<OutlineItem>, QuerySurfaceError> {
    let view = snapshot
        .global_view()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let spans = view
        .spans()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let table = graphzero_store::store::symbol_table::SymbolTable::from_view(&view)
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let blob_hashes = view
        .coverage()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?
        .blob_hashes;
    let hash_for_path = snapshot
        .path_records()
        .find(|(_, r)| r.path == rel)
        .map(|(h, _)| h.to_hex());
    outline_items_for_path(
        snapshot,
        rel,
        &table,
        &spans,
        blob_hashes,
        hash_for_path.as_deref(),
    )
}

fn outline_from_disk(
    snapshot: &Snapshot,
    rel: &str,
) -> Result<Vec<OutlineItem>, QuerySurfaceError> {
    let defs = snapshot
        .refresh_file(rel)
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    if defs.is_empty() {
        return Ok(Vec::new());
    }
    let repo = snapshot
        .repo_root
        .as_ref()
        .ok_or(QuerySurfaceError::MissingArgument("repo"))?;
    let content = std::fs::read(repo.join(rel)).map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    outline_items_from_defs(&content, &defs)
}

fn outline_items_from_defs(
    content: &[u8],
    defs: &[(String, String, u32, u32)],
) -> Result<Vec<OutlineItem>, QuerySurfaceError> {
    let mut outline = Vec::new();
    for (name, hash_hex, start, end) in defs {
        let evidence_ref = graphzero_store::store::refs::blob_span_ref(hash_hex, *start, *end);
        if evidence_ref.is_empty() {
            return Err(QuerySurfaceError::EvidenceMissing);
        }
        let (sl, el) = byte_span_to_lines(content, *start, *end);
        outline.push(OutlineItem {
            name: name.clone(),
            kind: "function".into(),
            evidence_ref,
            source: "tier_a".into(),
            start_line: Some(sl),
            end_line: Some(el),
        });
    }
    Ok(outline)
}

pub fn format_delta_budget_one(comp: &DeltaComputation, skeletons: &[String]) -> String {
    let since = &comp.since;
    if comp.changed.is_empty() && comp.added.is_empty() && comp.removed.is_empty() {
        return format!(
            "delta: unchanged since={since} files={}",
            comp.unchanged_count
        );
    }
    let mut lines = vec![format!(
        "delta: since={since} changed={} added={} removed={}",
        comp.changed.len(),
        comp.added.len(),
        comp.removed.len()
    )];
    lines.extend(skeletons.iter().take(MAX_SKELETON_FILES).cloned());
    lines.join("\n")
}
