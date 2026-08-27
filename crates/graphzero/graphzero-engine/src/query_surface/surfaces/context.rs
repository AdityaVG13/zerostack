use graphzero_store::Snapshot;
use graphzero_store::store::query::snap;
use serde_json::json;

use super::super::QuerySurfaceRouter;
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn context(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let query = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("query"))?;
        let capsule = snap(snapshot, query, budget, req.session.as_deref(), false)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(query.into()))?;
        let mut refs_footer: Vec<String> = capsule
            .destinations
            .iter()
            .map(|d| d.evidence_ref.clone())
            .collect();
        refs_footer.sort();
        refs_footer.dedup();
        if refs_footer.is_empty() && !capsule.destinations.is_empty() {
            refs_footer = capsule
                .destinations
                .iter()
                .map(|d| d.destination_ref.clone())
                .collect();
        }
        let capsule_json_str = capsule.to_json(Some(&snapshot.store_root));
        let truncated = capsule.ledger.truncated;
        let coverage = CoverageFooter {
            tier_a: capsule.coverage.tier_a,
            tier_b: capsule.coverage.tier_b,
            tier_c: capsule.coverage.tier_c,
            freshness_verified: capsule.coverage.freshness_verified,
            snapshot_id: capsule.snapshot_id,
        };
        let (capsule_field, full_ref) = (
            Some(serde_json::from_str(&capsule_json_str).unwrap_or_else(|_| json!({}))),
            None,
        );
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "context".into(),
            coverage,
            capsule: capsule_field,
            refs_footer,
            full_ref,
            truncated: Some(truncated),
            ..Default::default()
        })
    }
}
