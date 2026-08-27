use crate::accounting::accounting_for_evidence_refs;

use graphzero_store::Snapshot;
use graphzero_store::store::absence::{AbsenceConfig, absence};
use graphzero_store::store::query::QueryEngine;

use super::super::QuerySurfaceRouter;
use super::super::helpers::validate_edge_refs;
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn symbol(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let capsule = QueryEngine::warm(snapshot, name, budget)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
        let coverage = Self::footer(snapshot, &capsule)?;
        if capsule.matches.is_empty() {
            let cert = absence(snapshot, name, AbsenceConfig::default())
                .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
            return Ok(QuerySurfaceResponse {
                schema_version: 1,
                surface: "symbol".into(),
                coverage,
                absence_certificate: serde_json::from_str(&cert.to_json()).ok(),
                error: Some("SYMBOL_NOT_FOUND".into()),
                ..Default::default()
            });
        }
        let m = &capsule.matches[0];
        let decl_ref = graphzero_store::store::query::locate_shell_for_name(snapshot, name)
            .or_else(|| m.defs.first().map(|d| d.evidence_ref.clone()))
            .ok_or(QuerySurfaceError::EvidenceMissing)?;
        validate_edge_refs(&m.edges)?;
        let accounting = accounting_for_evidence_refs(
            snapshot,
            "orient_symbol",
            [&decl_ref],
            "orient selected one declaration ref instead of reading the full indexed repository",
        );
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "symbol".into(),
            coverage,
            decl_ref: Some(decl_ref),
            symbol: Some(m.name.clone()),
            accounting: Some(accounting),
            ..Default::default()
        })
    }
}
