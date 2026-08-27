//! P4.4 why-graph passive ingestion (schema v1, evidence, replay).

pub mod connectors;
pub mod edge_id;
pub mod evidence;
pub mod ingest;
pub mod redaction;
pub mod schema;
pub mod store;

pub use connectors::{ConnectorConfig, ConnectorStatus};
pub use evidence::{expand_evidence_ref, validate_evidence_refs};
pub use ingest::{
    CommitTouchedEntity, IngestReport, ingest_commit_fixture, ingest_commit_metadata_fixture,
    ingest_pr_issue_fixture, ingest_status, ingest_trace_fixture, ingest_unresolved_node_edge,
    replay_golden,
};
pub use schema::{
    ConnectorAvailability, ProvenanceSource, ProvenanceSourceKind, RedactionState, SCHEMA_VERSION,
    WhyEdge, WhyQueryManifest,
};
pub use store::{WhyChainEntry, WhyLedger, WhyStore, build_why_chain_for_node};
