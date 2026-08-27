//! Query surface handlers: thin dispatcher plus per-surface modules.

mod callers;
mod changes;
mod context;
mod deps;
mod hot;
mod outline;
mod reading;
mod search;
mod symbol;
mod word;

use graphzero_store::Snapshot;
use graphzero_store::store::query::Capsule;

use super::QuerySurfaceRouter;
use super::types::*;

impl QuerySurfaceRouter {
    pub fn execute(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let surface = QuerySurface::parse_surface(&req.surface)
            .ok_or_else(|| QuerySurfaceError::UnknownSurface(req.surface.clone()))?;
        let budget = req.budget.unwrap_or(1);
        let response = Self::route(snapshot, req, surface, budget)?;
        // RACC caching contract: every emitted fact must be a deterministic
        // function of (snapshot, operator, args).
        crate::deterministic_facts::debug_assert_deterministic_facts(&req.surface, &response);
        Ok(response)
    }

    fn route(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        surface: QuerySurface,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        match surface {
            QuerySurface::Symbol => Self::symbol(snapshot, req, budget),
            QuerySurface::Callers => Self::callers(snapshot, req, budget),
            QuerySurface::Deps => Self::deps(snapshot, req, budget),
            QuerySurface::Outline => Self::outline(snapshot, req, budget),
            QuerySurface::Context => Self::context(snapshot, req, budget),
            QuerySurface::Hot => Self::hot(snapshot, req, budget),
            QuerySurface::Changes => Self::changes(snapshot, req, budget),
            QuerySurface::Word => Self::word(snapshot, req, budget),
            QuerySurface::Search => Self::search(snapshot, req, budget),
            QuerySurface::Locate => Self::locate_surface(snapshot, req, budget),
            QuerySurface::Delta => Self::delta(snapshot, req, budget),
            QuerySurface::Recall => Self::recall(snapshot, req, budget),
            QuerySurface::Callpath => Self::callpath(snapshot, req, budget),
            QuerySurface::ReadingSet => Self::reading_set(snapshot, req, budget),
            QuerySurface::RgL1 => Self::rg_l1(snapshot, req, budget),
            QuerySurface::RgL2 => Self::rg_l2(snapshot, req, budget),
            QuerySurface::RgL3 => Self::rg_l3(snapshot, req, budget),
            QuerySurface::RgL4 => Self::rg_l4(snapshot, req, budget),
        }
    }

    /// Shared coverage footer for capsule-backed surfaces (and helpers).
    /// `pub(crate)` so per-surface submodules and sibling `helpers` can call it.
    pub(crate) fn footer(
        snapshot: &Snapshot,
        capsule: &Capsule,
    ) -> Result<CoverageFooter, QuerySurfaceError> {
        Ok(CoverageFooter {
            tier_a: capsule.tier_a,
            tier_b: capsule.tier_b,
            tier_c: capsule.tier_c,
            freshness_verified: snapshot.freshness_verified(),
            snapshot_id: capsule.snapshot_id,
        })
    }
}
