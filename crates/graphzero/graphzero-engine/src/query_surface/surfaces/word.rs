use graphzero_store::Snapshot;
use graphzero_store::store::query::QueryEngine;

use super::super::QuerySurfaceRouter;
use super::super::helpers::{empty_capsule, search_hits};
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn word(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let needle = req
            .query
            .as_deref()
            .or(req.name.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("query"))?;
        let hits = search_hits(snapshot, needle, budget)?;
        let capsule = QueryEngine::warm(snapshot, needle, budget)
            .unwrap_or_else(|_| empty_capsule(needle, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "word".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            hits,
            ..Default::default()
        })
    }
}
