//! Verifies post-edit assertions against graph evidence.

mod evidence_graph;
mod parse;
mod persist;
mod report;
mod verify;

pub use evidence_graph::append_verify_evidence_graph;
pub use parse::{ClaimKind, supported_claim_kinds_csv};
pub use report::{ClaimCertificate, ClaimVerifyResult, SurvivingSpan};
pub use verify::{ClaimVerifyConfig, verify_claim};
