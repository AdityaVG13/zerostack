use graphzero_store::Snapshot;

use super::super::QuerySurfaceRouter;
use super::super::helpers::tier_c_surface;
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn hot(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        tier_c_surface(
            snapshot,
            "hot",
            req.query.as_deref().unwrap_or("hot"),
            budget,
        )
    }
}
