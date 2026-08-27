use graphzero_store::Snapshot;
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::symbol_table::SymbolTable;

use super::super::QuerySurfaceRouter;
use super::super::helpers::{empty_capsule, outline_items_for_path};
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn outline(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        _budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        use graphzero_store::store::query::{
            StalenessVerdict, blob_staleness_verdict, path_record_for_rel,
        };

        use super::super::skeleton::format_outline_skeleton;

        let rel = req
            .path
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("path"))?;
        let view = snapshot
            .global_view()
            .map_err(|_| QuerySurfaceError::MissingArgument("path"))?;
        let spans = view
            .spans()
            .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
        let table =
            SymbolTable::from_view(&view).map_err(|_| QuerySurfaceError::EvidenceMissing)?;
        let blob_hashes = view
            .coverage()
            .map_err(|_| QuerySurfaceError::EvidenceMissing)?
            .blob_hashes;
        let hash_for_path = snapshot
            .path_records()
            .find(|(_, r)| r.path == rel)
            .map(|(h, _)| h.to_hex());
        let outline = outline_items_for_path(
            snapshot,
            rel,
            &table,
            &spans,
            blob_hashes,
            hash_for_path.as_deref(),
        )?;
        let mut path_stale = false;
        if let (Some(repo), Some((hash_hex, rec))) = (
            snapshot.repo_root.as_ref(),
            path_record_for_rel(snapshot, rel),
        ) {
            path_stale = matches!(
                blob_staleness_verdict(repo, rel, &hash_hex, rec),
                StalenessVerdict::Stale | StalenessVerdict::Missing | StalenessVerdict::Unreadable
            );
        }
        let mem_idx = graphzero_store::MemoryIndex::load(&snapshot.store_root).unwrap_or_default();
        let mem_hints = mem_idx.hints_for_path(snapshot, rel, 2);
        let mut skeleton = graphzero_store::attach_memory_to_skeleton(
            &format_outline_skeleton(rel, &outline),
            &mem_hints,
        );
        if path_stale {
            skeleton = format!("stale:{skeleton}");
        }
        let capsule =
            QueryEngine::warm(snapshot, rel, 256).unwrap_or_else(|_| empty_capsule(rel, snapshot));
        let mut coverage = Self::footer(snapshot, &capsule)?;
        if path_stale {
            coverage.freshness_verified = true;
        }
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "outline".into(),
            coverage,
            outline,
            skeleton,
            ..Default::default()
        })
    }

    pub(super) fn recall(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        _budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let target = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .or(req.path.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("target"))?;
        let idx = graphzero_store::MemoryIndex::load(&snapshot.store_root)
            .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
        let facts = idx.facts_for_target(target);
        let rows: Vec<serde_json::Value> = facts
            .iter()
            .map(|f| {
                serde_json::json!({
                    "id": f.id,
                    "kind": f.kind.as_str(),
                    "text": f.text,
                    "anchors": f.anchors,
                    "ts": f.ts,
                    "ref": graphzero_store::mem_ref(&f.id),
                })
            })
            .collect();
        let capsule = QueryEngine::warm(snapshot, target, 1)
            .unwrap_or_else(|_| empty_capsule(target, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "recall".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            rows,
            symbol: Some(target.to_string()),
            ..Default::default()
        })
    }
}
