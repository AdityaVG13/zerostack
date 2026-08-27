//! FR-006: expandable gz:// evidence refs.

use std::path::Path;

use anyhow::{Context, Result};
use graphzero_store::{ExpandResolver, GzRef};

pub fn evidence_ref_from_blob(hash_hex: &str) -> String {
    format!("gz://blob/{hash_hex}")
}

pub fn store_text_evidence(
    store_root: &Path,
    repo_root: Option<&Path>,
    text: &str,
) -> Result<String> {
    use graphzero_store::store::blob_store::BlobStore;
    let blobs = BlobStore::open(store_root)?;
    let hash = blobs.put(text.as_bytes())?;
    let _ = ExpandResolver::new(store_root, repo_root).context("expand resolver")?;
    Ok(evidence_ref_from_blob(&hash.to_hex()))
}

pub fn expand_evidence_ref(
    store_root: &Path,
    repo_root: Option<&Path>,
    reference: &str,
) -> Result<Vec<u8>> {
    let gz = GzRef::parse(reference).context("parse gz ref")?;
    let resolver = ExpandResolver::new(store_root, repo_root)?;
    Ok(resolver.resolve(&gz, reference)?.bytes)
}

pub fn validate_evidence_refs(
    store_root: &Path,
    repo_root: Option<&Path>,
    refs: &[String],
) -> Result<()> {
    if refs.is_empty() {
        anyhow::bail!("evidence_refs empty");
    }
    for r in refs {
        expand_evidence_ref(store_root, repo_root, r)
            .with_context(|| format!("evidence not expandable: {r}"))?;
    }
    Ok(())
}

/// Confidence is an ordinal source-strength score, not a probability.
/// Acceptable values are finite numbers within 0.0..=1.0.
pub fn validate_confidence_score(confidence: f32) -> Result<f32, String> {
    if !confidence.is_finite() {
        Err("confidence must be finite".into())
    } else if !(0.0..=1.0).contains(&confidence) {
        Err("confidence must be between 0.0 and 1.0".into())
    } else {
        Ok(confidence)
    }
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-why/evidence_tests.rs"]
mod tests;
