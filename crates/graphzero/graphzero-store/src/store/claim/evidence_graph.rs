//! Verification graph augmentation produced in the same domain execution as verify.

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::ContentHash;
use crate::store::csr::edge_kind;
use crate::store::delta_log::{DeltaEntry, DeltaLog, entry_type};
use crate::store::format::symbol_kind;
use crate::store::lock::WriterLock;
use crate::store::query::encode_edge_with_meta;
use crate::store::query::encode_symbol;
use crate::store::refs::{Fragment, GzRef};

use super::ClaimVerifyResult;

/// Append a verification evidence node/edge to the WAL and return graph JSON for
/// the verify response. Runs in the same domain execution as `verify_claim`
/// (not a second claim verification).
pub fn append_verify_evidence_graph(
    store_root: &std::path::Path,
    result: &ClaimVerifyResult,
    edge_source: &str,
) -> Result<Option<Value>> {
    let evidence_ref = result.evidence_ref.as_deref().or_else(|| {
        result
            .surviving_spans
            .first()
            .map(|span| span.evidence_ref.as_str())
    });
    let Some(evidence_ref) = evidence_ref else {
        return Ok(None);
    };
    let gz = GzRef::parse(evidence_ref).context("parse verify evidence ref")?;
    let GzRef::Blob { hash, fragment } = gz else {
        return Ok(None);
    };
    let Fragment::Bytes { start, end } = fragment else {
        return Ok(None);
    };
    if ContentHash::from_hex(&hash).is_none() {
        return Ok(None);
    }

    let safe_target = result
        .target
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let node = format!(
        "<verify:{}:{}:{}:{}-{}>",
        result.claim_kind,
        safe_target,
        &hash[..hash.len().min(12)],
        start,
        end
    );
    let kind = if result.verified {
        edge_kind::VERIFICATION_PASSED
    } else {
        edge_kind::VERIFICATION_FAILED
    };
    let confidence = if result.unknown_reason.is_some() {
        0
    } else {
        255
    };

    let _lock = WriterLock::acquire(store_root)?;
    let mut log = DeltaLog::open(store_root)?;
    // Synthetic blob identity: never reuse the evidence content hash. Pending
    // WAL merge treats any blob keyed in pending as dirty-replacement and
    // drops all base defs/edges for that blob on compaction — annotating the
    // real evidence blob wiped symbols like `alpha` and made later verify
    // adapters report target_not_found (graphzero-dispatcher-verify-adapter-
    // divergence-zk92b).
    let synthetic = ContentHash::of(format!("graphzero.verify\0{node}").as_bytes());
    log.append(DeltaEntry {
        entry_type: entry_type::SYMBOL,
        blob_hash: synthetic.0,
        payload: encode_symbol(&node, symbol_kind::OTHER, 2, start as u32, end as u32)?,
    })?;
    log.append(DeltaEntry {
        entry_type: entry_type::EDGE,
        blob_hash: synthetic.0,
        payload: encode_edge_with_meta(
            &node,
            &result.target,
            kind,
            confidence,
            start as u32,
            end as u32,
            Some(edge_source),
        )?,
    })?;
    // Do not append COVERAGE for the source evidence blob. Verify nodes are
    // synthetic annotations; rewriting coverage to tier-C-only poisons reopen
    // (partial_coverage) and makes later adapters diverge after the first
    // successful verify on a shared store.
    log.commit()?;

    Ok(Some(json!({
        "node": node,
        "edge": {
            "src": node,
            "dst": result.target,
            "kind": if result.verified { "verification_passed" } else { "verification_failed" },
            "evidence_ref": evidence_ref,
            "source": edge_source,
            "confidence": confidence as f64 / 255.0
        }
    })))
}
