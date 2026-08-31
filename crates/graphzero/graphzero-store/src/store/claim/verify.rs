use std::collections::BTreeSet;

use anyhow::Result;

use super::super::absence::{AbsenceAnswer, AbsenceConfig, AnswerClass, absence};
use super::super::csr::{CsrAdjacency, ReverseIndex, edge_kind};
use super::super::format::SpanEntry;
use super::super::query::Snapshot;
use super::super::refs::blob_span_ref;
use super::super::symbol_table::SymbolTable;
use super::report::{
    claim_certificate_from_absence, claim_result_from_survivors, claim_result_target_not_found,
    claim_result_unknown_coverage,
};
use super::{ClaimCertificate, ClaimKind, ClaimVerifyResult, SurvivingSpan};

#[derive(Clone, Copy, Debug)]
pub struct ClaimVerifyConfig {
    pub tier_a_threshold: f64,
    pub check_freshness: bool,
}

impl Default for ClaimVerifyConfig {
    fn default() -> Self {
        Self {
            tier_a_threshold: 0.99,
            check_freshness: true,
        }
    }
}

/// Verify a caller assertion against the indexed graph.
pub fn verify_claim(
    snapshot: &Snapshot,
    kind: ClaimKind,
    target: &str,
    config: ClaimVerifyConfig,
) -> Result<ClaimVerifyResult> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("empty claim target");
    }
    let mut result = match kind {
        ClaimKind::NoRemainingCallers => verify_incoming_edges(
            snapshot,
            target,
            config,
            edge_kind::CALLS,
            "calls",
            "callers",
        )?,
        ClaimKind::NoOutgoingCalls => verify_outgoing_edges(
            snapshot,
            target,
            config,
            edge_kind::CALLS,
            "calls",
            "outgoing calls",
        )?,
        ClaimKind::NoRemainingReferences => verify_incoming_edges(
            snapshot,
            target,
            config,
            edge_kind::REFS,
            "refs",
            "references",
        )?,
        ClaimKind::SymbolRemoved => verify_symbol_removed(snapshot, target, config)?,
        ClaimKind::NoRemainingDependencies => {
            verify_incoming_dependencies(snapshot, target, config)?
        }
    };
    attach_why_provenance(snapshot, &mut result);
    Ok(result)
}

fn attach_why_provenance(snapshot: &Snapshot, result: &mut ClaimVerifyResult) {
    let mut seen = BTreeSet::new();
    let mut refs: Vec<String> = Vec::new();
    if let Some(r) = result.evidence_ref.as_ref()
        && seen.insert(r.clone())
    {
        refs.push(r.clone());
    }
    for span in &result.surviving_spans {
        if seen.insert(span.evidence_ref.clone()) {
            refs.push(span.evidence_ref.clone());
        }
    }
    let mut why = Vec::new();
    for r in refs {
        if let Some(p) = super::super::provenance::why_for_evidence_ref(&snapshot.store_root, &r) {
            why.push(p);
        }
    }
    result.provenance = why;
}

fn claim_presence(
    snapshot: &Snapshot,
    target: &str,
    config: ClaimVerifyConfig,
) -> Result<(AbsenceAnswer, ClaimCertificate)> {
    let presence = absence(
        snapshot,
        target,
        AbsenceConfig {
            tier_a_threshold: config.tier_a_threshold,
            check_freshness: config.check_freshness,
        },
    )?;
    let certificate = claim_certificate_from_absence(&presence);
    Ok((presence, certificate))
}

fn gate_target_presence(
    claim_kind: &str,
    target: &str,
    cert: &ClaimCertificate,
    presence: &AbsenceAnswer,
) -> Option<ClaimVerifyResult> {
    if presence.class == AnswerClass::Unknown {
        return Some(claim_result_unknown_coverage(
            claim_kind,
            target,
            cert.clone(),
            presence,
        ));
    }
    if presence.class == AnswerClass::Absent {
        return Some(claim_result_target_not_found(
            claim_kind,
            target,
            cert.clone(),
            &format!("unknown: target symbol {:?} not in graph", target),
            "target_not_found",
        ));
    }
    None
}

fn target_id_or_not_found(
    table: &SymbolTable,
    claim_kind: &str,
    target: &str,
    cert: &ClaimCertificate,
) -> Result<Option<ClaimVerifyResult>> {
    if table.get(target).is_some() {
        return Ok(None);
    }
    Ok(Some(claim_result_target_not_found(
        claim_kind,
        target,
        cert.clone(),
        &format!("unknown: symbol table miss for {:?}", target),
        "target_not_found",
    )))
}

fn span_from_edge(
    table: &SymbolTable,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    src: u32,
    edge_idx: usize,
    edge: super::super::csr::Edge,
    edge_label: &str,
) -> Result<Option<SurvivingSpan>> {
    let ev: SpanEntry = evidence.get(edge_idx).copied().unwrap_or_default();
    let from_symbol = table.name(src).unwrap_or("").to_string();
    let to_symbol = table.name(edge.target).unwrap_or("").to_string();
    let hash_hex = crate::hex_blob_hash(blob_hashes, ev.blob_idx)?;
    let evidence_ref = blob_span_ref(&hash_hex, ev.start, ev.end);
    if evidence_ref.is_empty() {
        return Ok(None);
    }
    Ok(Some(SurvivingSpan {
        kind: edge_label.into(),
        from_symbol,
        to_symbol,
        evidence_ref,
        confidence: edge.confidence as f64 / 255.0,
        source: "tier_a".into(),
    }))
}

fn collect_incoming_edges(
    table: &SymbolTable,
    csr: &CsrAdjacency,
    reverse: &ReverseIndex,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    target_id: u32,
    edge_kind_filter: u8,
    edge_label: &str,
) -> Result<Vec<SurvivingSpan>> {
    let mut surviving = Vec::new();
    let mut seen_refs = BTreeSet::new();

    for &(src, edge_idx) in reverse.callers(target_id) {
        let edge_idx = edge_idx as usize;
        let edge = csr
            .edges(src)
            .nth(edge_idx - csr.edge_base(src))
            .filter(|e| e.target == target_id && e.kind == edge_kind_filter);
        let Some(edge) = edge else {
            continue;
        };
        let Some(span) = span_from_edge(
            table,
            evidence,
            blob_hashes,
            src,
            edge_idx,
            edge,
            edge_label,
        )?
        else {
            continue;
        };
        if seen_refs.insert(span.evidence_ref.clone()) {
            surviving.push(span);
        }
    }
    Ok(surviving)
}

fn collect_outgoing_edges(
    table: &SymbolTable,
    csr: &CsrAdjacency,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    target_id: u32,
    edge_kind_filter: u8,
    edge_label: &str,
) -> Result<Vec<SurvivingSpan>> {
    let mut surviving = Vec::new();
    let mut seen_refs = BTreeSet::new();
    let base = csr.edge_base(target_id);

    for (i, edge) in csr.edges(target_id).enumerate() {
        if edge.kind != edge_kind_filter {
            continue;
        }
        let edge_idx = base + i;
        let Some(span) = span_from_edge(
            table,
            evidence,
            blob_hashes,
            target_id,
            edge_idx,
            edge,
            edge_label,
        )?
        else {
            continue;
        };
        if seen_refs.insert(span.evidence_ref.clone()) {
            surviving.push(span);
        }
    }
    Ok(surviving)
}

#[derive(Clone, Copy)]
enum EdgeScanDirection {
    Incoming,
    Outgoing,
}

/// Shared parameters for the incoming/outgoing edge claim scans.
struct EdgeScan<'a> {
    claim_kind: &'a str,
    target: &'a str,
    cert: ClaimCertificate,
    edge_kind_filter: u8,
    edge_label: &'a str,
    refuted_label: &'a str,
    target_evidence_ref: Option<String>,
}

fn scan_edges(
    snapshot: &Snapshot,
    direction: EdgeScanDirection,
    scan: EdgeScan,
) -> Result<ClaimVerifyResult> {
    let EdgeScan {
        claim_kind,
        target,
        cert,
        edge_kind_filter,
        edge_label,
        refuted_label,
        target_evidence_ref,
    } = scan;
    let view = snapshot.global_view()?;
    let table = SymbolTable::from_view(&view)?;
    if let Some(blocked) = target_id_or_not_found(&table, claim_kind, target, &cert)? {
        return Ok(blocked);
    }
    let target_id = table.get(target).expect("gated present symbol");

    let csr = CsrAdjacency::new(view.edges()?);
    let evidence = view.edge_evidence()?;
    let blob_hashes = view.coverage()?.blob_hashes;
    let (surviving, relation) = match direction {
        EdgeScanDirection::Incoming => (
            collect_incoming_edges(
                &table,
                &csr,
                snapshot.reverse_index()?,
                &evidence,
                blob_hashes,
                target_id,
                edge_kind_filter,
                edge_label,
            )?,
            "remain for",
        ),
        EdgeScanDirection::Outgoing => (
            collect_outgoing_edges(
                &table,
                &csr,
                &evidence,
                blob_hashes,
                target_id,
                edge_kind_filter,
                edge_label,
            )?,
            "from",
        ),
    };

    let verified_summary = format!(
        "verified: no tier-A {refuted_label} {relation} {:?} (tier-A {:.1}% fresh, snapshot {})",
        target, cert.tier_a_pct, cert.snapshot_id
    );
    Ok(claim_result_from_survivors(
        claim_kind,
        target,
        cert,
        surviving,
        &verified_summary,
        refuted_label,
        target_evidence_ref,
    ))
}

fn verify_symbol_removed(
    snapshot: &Snapshot,
    target: &str,
    config: ClaimVerifyConfig,
) -> Result<ClaimVerifyResult> {
    let claim_kind = ClaimKind::SymbolRemoved.as_str();
    let (presence, cert) = claim_presence(snapshot, target, config)?;

    match presence.class {
        AnswerClass::Absent => Ok(ClaimVerifyResult {
            schema_version: 1,
            verified: true,
            claim_kind: claim_kind.to_string(),
            target: target.to_string(),
            summary: format!(
                "verified: symbol {:?} absent from graph (tier-A {:.1}% fresh, snapshot {})",
                target, cert.tier_a_pct, cert.snapshot_id
            ),
            certificate: cert,
            evidence_ref: None,
            surviving_spans: Vec::new(),
            unknown_reason: None,
            provenance: Vec::new(),
        }),
        AnswerClass::Unknown => Ok(claim_result_unknown_coverage(
            claim_kind, target, cert, &presence,
        )),
        AnswerClass::Present => {
            let mut surviving = Vec::new();
            if let Some(evidence_ref) = presence.evidence_ref.filter(|r| !r.is_empty()) {
                surviving.push(SurvivingSpan {
                    kind: "def".into(),
                    from_symbol: target.to_string(),
                    to_symbol: target.to_string(),
                    evidence_ref,
                    confidence: 1.0,
                    source: "tier_a".into(),
                });
            }
            Ok(ClaimVerifyResult {
                schema_version: 1,
                verified: false,
                claim_kind: claim_kind.to_string(),
                target: target.to_string(),
                summary: format!("refuted: symbol {:?} still present in graph", target),
                certificate: cert,
                evidence_ref: surviving.first().map(|span| span.evidence_ref.clone()),
                surviving_spans: surviving,
                unknown_reason: None,
                provenance: Vec::new(),
            })
        }
    }
}

fn verify_incoming_edges(
    snapshot: &Snapshot,
    target: &str,
    config: ClaimVerifyConfig,
    edge_kind_filter: u8,
    edge_label: &str,
    refuted_label: &str,
) -> Result<ClaimVerifyResult> {
    let claim_kind = match edge_kind_filter {
        edge_kind::CALLS => ClaimKind::NoRemainingCallers.as_str(),
        edge_kind::REFS => ClaimKind::NoRemainingReferences.as_str(),
        _ => "incoming_edge_claim",
    };
    let (presence, cert) = claim_presence(snapshot, target, config)?;

    if let Some(blocked) = gate_target_presence(claim_kind, target, &cert, &presence) {
        return Ok(blocked);
    }

    scan_edges(
        snapshot,
        EdgeScanDirection::Incoming,
        EdgeScan {
            claim_kind,
            target,
            cert,
            edge_kind_filter,
            edge_label,
            refuted_label,
            target_evidence_ref: presence.evidence_ref,
        },
    )
}

fn verify_incoming_dependencies(
    snapshot: &Snapshot,
    target: &str,
    config: ClaimVerifyConfig,
) -> Result<ClaimVerifyResult> {
    let claim_kind = ClaimKind::NoRemainingDependencies.as_str();
    let (presence, cert) = claim_presence(snapshot, target, config)?;

    if let Some(blocked) = gate_target_presence(claim_kind, target, &cert, &presence) {
        return Ok(blocked);
    }

    scan_incoming_dependencies(snapshot, claim_kind, target, cert, presence.evidence_ref)
}

fn scan_incoming_dependencies(
    snapshot: &Snapshot,
    claim_kind: &str,
    target: &str,
    cert: ClaimCertificate,
    target_evidence_ref: Option<String>,
) -> Result<ClaimVerifyResult> {
    let view = snapshot.global_view()?;
    let table = SymbolTable::from_view(&view)?;
    if let Some(blocked) = target_id_or_not_found(&table, claim_kind, target, &cert)? {
        return Ok(blocked);
    }
    let target_id = table.get(target).expect("gated present symbol");

    let csr = CsrAdjacency::new(view.edges()?);
    let evidence = view.edge_evidence()?;
    let blob_hashes = view.coverage()?.blob_hashes;

    let mut surviving = Vec::new();
    let mut seen_refs = BTreeSet::new();
    let kinds = [
        (edge_kind::CALLS, "calls"),
        (edge_kind::REFS, "refs"),
        (edge_kind::IMPORTS, "imports"),
    ];
    for (kind_filter, label) in kinds {
        for span in collect_incoming_edges(
            &table,
            &csr,
            snapshot.reverse_index()?,
            &evidence,
            blob_hashes,
            target_id,
            kind_filter,
            label,
        )? {
            if seen_refs.insert(span.evidence_ref.clone()) {
                surviving.push(span);
            }
        }
    }

    let verified_summary = format!(
        "verified: no tier-A incoming dependencies remain for {:?} (tier-A {:.1}% fresh, snapshot {})",
        target, cert.tier_a_pct, cert.snapshot_id
    );
    Ok(claim_result_from_survivors(
        claim_kind,
        target,
        cert,
        surviving,
        &verified_summary,
        "dependencies",
        target_evidence_ref,
    ))
}

fn verify_outgoing_edges(
    snapshot: &Snapshot,
    target: &str,
    config: ClaimVerifyConfig,
    edge_kind_filter: u8,
    edge_label: &str,
    refuted_label: &str,
) -> Result<ClaimVerifyResult> {
    let claim_kind = ClaimKind::NoOutgoingCalls.as_str();
    let (presence, cert) = claim_presence(snapshot, target, config)?;

    if let Some(blocked) = gate_target_presence(claim_kind, target, &cert, &presence) {
        return Ok(blocked);
    }

    scan_edges(
        snapshot,
        EdgeScanDirection::Outgoing,
        EdgeScan {
            claim_kind,
            target,
            cert,
            edge_kind_filter,
            edge_label,
            refuted_label,
            target_evidence_ref: presence.evidence_ref,
        },
    )
}
