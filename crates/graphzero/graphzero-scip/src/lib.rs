//! P3.1 Tier-B SCIP ingest -- decode SCIP protobuf into witness-labeled edges.

pub mod decode;
pub mod ingest;
pub mod lsp;
pub mod types;

pub mod publish;
pub use decode::decode_scip_bytes;
pub use ingest::{ScipIngestPlan, apply_scip_to_index, scip_facts_from_bytes};
pub use publish::{ingest_scip_publish, tier_b_count_from_data};
pub use types::{ScipDecoded, TierBEdge, TierBResolution, TierBSource};
