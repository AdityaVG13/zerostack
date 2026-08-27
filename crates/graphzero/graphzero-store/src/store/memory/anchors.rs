//! Anchor/path/symbol resolution and drift checks against Snapshot.

use anyhow::{Result, bail};

use crate::store::format::SpanEntry;
use crate::store::query::LocateIndex;
use crate::store::query::{PathRecord, Snapshot, path_record_for_rel, span_range};
use crate::store::refs::blob_span_ref;
use crate::store::symbol_table::SymbolTable;

use super::types::AnchorResolution;

pub(super) fn looks_like_path(anchor: &str) -> bool {
    anchor.contains('/') || anchor.ends_with(".rs") || anchor.ends_with(".ts")
}

pub(super) fn resolve_anchors(
    snapshot: &Snapshot,
    anchors: &[String],
) -> Result<Vec<AnchorResolution>> {
    let locate = LocateIndex::build(snapshot).ok();
    let view = snapshot.global_view().ok();
    let mut table = None;
    let mut spans: Option<Vec<SpanEntry>> = None;
    let mut blob_hashes = None;
    if let Some(v) = view.as_ref()
        && let Ok(t) = SymbolTable::from_view(v)
    {
        table = Some(t);
        spans = v.spans().ok().map(|s| s.into_owned());
        blob_hashes = v.coverage().ok().map(|c| c.blob_hashes);
    }
    anchors
        .iter()
        .map(|a| {
            resolve_one_anchor(
                snapshot,
                a,
                locate.as_ref(),
                table.as_ref(),
                spans.as_deref(),
                blob_hashes,
            )
        })
        .collect()
}

pub(super) fn checked_blob_hash(blob_hashes: &[[u8; 32]], idx: u32) -> Result<String> {
    let Some(hash) = blob_hashes.get(idx as usize) else {
        bail!(
            "span blob_idx {idx} out of range for {} blob hashes",
            blob_hashes.len()
        );
    };
    Ok(crate::fast_hex_32(hash))
}

fn resolve_one_anchor(
    snapshot: &Snapshot,
    anchor: &str,
    locate: Option<&LocateIndex>,
    table: Option<&SymbolTable>,
    spans: Option<&[SpanEntry]>,
    blob_hashes: Option<&[[u8; 32]]>,
) -> Result<AnchorResolution> {
    let mut res = AnchorResolution {
        anchor: anchor.to_string(),
        path: None,
        symbol: None,
        content_sha256: None,
        symbol_id: None,
        evidence_ref: None,
        drifted: false,
    };

    if looks_like_path(anchor) {
        res.path = Some(anchor.to_string());
        if let Some((hash_hex, rec)) = path_record_for_rel(snapshot, anchor) {
            res.drifted = path_drifted(snapshot, anchor, &hash_hex, rec);
            res.content_sha256 = Some(hash_hex);
        }
        return Ok(res);
    }

    res.symbol = Some(anchor.to_string());
    if let (Some(table), Some(spans), Some(blob_hashes)) = (table, spans, blob_hashes)
        && let Some(id) = table.get(anchor)
    {
        res.symbol_id = Some(id);
        if let Some(span) = span_range(spans, id).first() {
            let hash_hex = checked_blob_hash(blob_hashes, span.blob_idx)?;
            res.content_sha256 = Some(hash_hex.clone());
            let (ns, ne) = span.name_byte_range();
            res.evidence_ref = Some(blob_span_ref(&hash_hex, ns, ne));
            if let Some(path_rec) = snapshot.path_for_blob(&hash_hex) {
                res.path = Some(path_rec.path.clone());
                if let Some((h2, rec2)) = path_record_for_rel(snapshot, &path_rec.path) {
                    res.drifted = path_drifted(snapshot, &path_rec.path, &h2, rec2);
                }
            }
        }
    }
    if res.evidence_ref.is_none()
        && let Some(loc) = locate
        && let Some(&loc_id) = loc.symbol_to_loc.get(anchor)
        && let Some(entry) = loc.by_id.get(&loc_id)
    {
        res.evidence_ref = Some(entry.canonical_ref.clone());
        if let Some(p) = &entry.path {
            res.path = Some(p.clone());
        }
    }
    Ok(res)
}

pub(super) fn anchor_path_drifted_now(
    snapshot: &Snapshot,
    path: &str,
    res: &AnchorResolution,
) -> bool {
    if let Some(stored) = &res.content_sha256 {
        if let Some((h, rec)) = path_record_for_rel(snapshot, path) {
            return h != *stored || path_drifted(snapshot, path, &h, rec);
        }
        return true;
    }
    res.drifted
}

pub(super) fn path_drifted(
    snapshot: &Snapshot,
    rel: &str,
    indexed_hash: &str,
    rec: &PathRecord,
) -> bool {
    let Some(repo) = snapshot.repo_root.as_ref() else {
        return false;
    };
    use crate::store::query::{StalenessVerdict, blob_staleness_verdict};
    matches!(
        blob_staleness_verdict(repo, rel, indexed_hash, rec),
        StalenessVerdict::Stale | StalenessVerdict::Missing | StalenessVerdict::Unreadable
    )
}
