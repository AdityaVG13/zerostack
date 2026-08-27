use sha2::{Digest, Sha256};

use crate::schema::{ProvenanceSourceKind, WhyEdge, WhyRelation};

fn relation_tag(r: WhyRelation) -> &'static str {
    match r {
        WhyRelation::Introduced => "introduced",
        WhyRelation::Modified => "modified",
        WhyRelation::Discussed => "discussed",
        WhyRelation::Decided => "decided",
        WhyRelation::Reviewed => "reviewed",
    }
}

fn kind_tag(k: ProvenanceSourceKind) -> &'static str {
    match k {
        ProvenanceSourceKind::GitCommit => "git_commit",
        ProvenanceSourceKind::PrThread => "pr_thread",
        ProvenanceSourceKind::Issue => "issue",
        ProvenanceSourceKind::AgentTrace => "agent_trace",
    }
}

/// Deterministic edge ID (INV-PROOF-001).
pub fn compute_edge_id(edge: &WhyEdge) -> String {
    let node = edge.node_ref.as_deref().unwrap_or("");
    let evidence_digest = {
        let mut h = Sha256::new();
        for r in &edge.evidence_refs {
            h.update(r.as_bytes());
            h.update(b"\n");
        }
        graphzero_store::fast_hex(h.finalize().as_slice())
    };
    let canonical = format!(
        "v1|{}|{}|{}|{}|{}",
        kind_tag(edge.source.kind),
        edge.source.stable_id,
        node,
        relation_tag(edge.relation),
        evidence_digest
    );
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    format!(
        "why_{}",
        &graphzero_store::fast_hex(h.finalize().as_slice())[..32]
    )
}
