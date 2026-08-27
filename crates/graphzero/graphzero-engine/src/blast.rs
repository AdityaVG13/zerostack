//! P4.1 blast-radius intent queries (walking skeleton).

mod parse;
mod render;
mod traverse;
mod types;

pub use parse::{impact_before_edit, parse_intent};
pub use render::{
    blast_from_json, blast_to_json, blast_to_json_budget, blast_to_value_budget,
    resume_blast_cursor,
};
pub use traverse::{blast_radius, blast_radius_with_depth, retrieval_neighborhood};
pub use types::{
    BLAST_SCHEMA_VERSION, BlastCoverageFooter, BlastError, BlastIntentParse, BlastRadiusCapsule,
    BreakSite, CoveringTest, EdgeProvenance, PlannedEdit, PlannedImpact, RetrievalEdge,
    RetrievalNeighborhood, RetrievalNode, SilentRisk, SpeculativeBlastReport,
    SpeculativeBlastRequest,
};
