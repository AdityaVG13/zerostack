//! P4.2 claim verification: post-edit assertions checked against the graph
//! (vision pillar 3 — certified claims, blast radius run backwards).

mod evidence_graph;
mod parse;
mod persist;
mod report;
mod verify;

pub use evidence_graph::append_verify_evidence_graph;
pub use parse::{ClaimKind, supported_claim_kinds_csv};
pub use report::{ClaimCertificate, ClaimVerifyResult, SurvivingSpan};
pub use verify::{ClaimVerifyConfig, verify_claim};
