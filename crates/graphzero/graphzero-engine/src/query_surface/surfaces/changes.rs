use graphzero_store::Snapshot;

use super::super::QuerySurfaceRouter;
use super::super::helpers::tier_c_surface;
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn changes(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .unwrap_or("changes");
        tier_c_surface(snapshot, "changes", name, budget)
    }
}
