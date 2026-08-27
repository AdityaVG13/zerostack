use graphzero_store::Snapshot;
use graphzero_store::store::query::{LocateKind, QueryEngine, locate, locate_shell};

use super::super::QuerySurfaceRouter;
use super::super::helpers::{empty_capsule, merge_exact_symbol_search_hit, search_hits};
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn search(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let needle = req
            .query
            .as_deref()
            .or(req.name.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("query"))?;
        let mut hits = search_hits(snapshot, needle, budget)?;
        merge_exact_symbol_search_hit(snapshot, needle, budget, &mut hits);
        hits.sort_by(|a, b| a.content_sha256.cmp(&b.content_sha256));
        hits.dedup_by(|a, b| a.content_sha256 == b.content_sha256);
        super::super::frecency::rank_search_hits(snapshot, &mut hits);
        let capsule = QueryEngine::warm(snapshot, needle, budget)
            .unwrap_or_else(|_| empty_capsule(needle, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "search".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            hits,
            ..Default::default()
        })
    }

    pub(super) fn locate_surface(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let query = req
            .query
            .as_deref()
            .or(req.path.as_deref())
            .or(req.name.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("query"))?;
        let kind = if req.path.is_some() {
            LocateKind::Path
        } else {
            LocateKind::Auto
        };
        let capsule = locate(snapshot, query, kind)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(query.into()))?;
        let shell = locate_shell(&capsule);
        let warm = QueryEngine::warm(snapshot, query, budget)
            .unwrap_or_else(|_| empty_capsule(query, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "locate".into(),
            coverage: Self::footer(snapshot, &warm)?,
            decl_ref: Some(shell),
            full_ref: capsule.detail_ref.clone(),
            truncated: capsule.detail_ref.as_ref().map(|_| true),
            ..Default::default()
        })
    }
}
