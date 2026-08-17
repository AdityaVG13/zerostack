//! Three-tier golden artifacts. Encode the distinction; never paper over it.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::repo::{read_bytes, repo_root, sha256_hex};
use crate::spec_oracle::all_verifiers;

pub const MANIFEST_SCHEMA_VERSION: &str = "1.0.0";
pub const GOLDEN_DIR: &str = "conformance/golden";
pub const MANIFEST_REL: &str = "conformance/golden/manifest.v1.json";
pub const CHECKSUMS_REL: &str = "conformance/golden/checksums.sha256";
pub const TIER3_REL: &str = "conformance/golden/tier3/logical_dump.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquivalenceTier {
    Tier1Raw,
    Tier2Canonical,
    Tier3Logical,
}

impl EquivalenceTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tier1Raw => "Tier1Raw",
            Self::Tier2Canonical => "Tier2Canonical",
            Self::Tier3Logical => "Tier3Logical",
        }
    }

    pub fn parse(label: &str) -> Result<Self, String> {
        match label {
            "Tier1Raw" => Ok(Self::Tier1Raw),
            "Tier2Canonical" => Ok(Self::Tier2Canonical),
            "Tier3Logical" => Ok(Self::Tier3Logical),
            other => Err(format!("unknown equivalence tier {other}")),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManifestArtifact {
    pub fixture_id: String,
    pub tier: String,
    pub source: String,
    pub path: String,
    pub sha256: String,
    pub source_artifact_path: Option<String>,
    pub canonicalization_fn: Option<String>,
    pub equivalence_predicate: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GoldenManifest {
    pub schema_version: String,
    pub artifacts: Vec<ManifestArtifact>,
}

impl ManifestArtifact {
    pub fn tier(&self) -> Result<EquivalenceTier, String> {
        EquivalenceTier::parse(&self.tier)
    }
}

pub fn golden_root(root: &Path) -> std::path::PathBuf {
    root.join(GOLDEN_DIR)
}

pub fn load_manifest(root: &Path) -> Result<GoldenManifest, String> {
    let text = crate::repo::read_text(root, MANIFEST_REL)?;
    let manifest: GoldenManifest =
        serde_json::from_str(&text).map_err(|error| format!("manifest parse: {error}"))?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "manifest schema_version is {}, expected {MANIFEST_SCHEMA_VERSION}",
            manifest.schema_version
        ));
    }
    if manifest.artifacts.is_empty() {
        return Err("manifest.artifacts is empty".into());
    }
    let mut ids: Vec<&str> = manifest
        .artifacts
        .iter()
        .map(|item| item.fixture_id.as_str())
        .collect();
    let sorted = {
        let mut copy = ids.clone();
        copy.sort_unstable();
        copy
    };
    if ids != sorted {
        return Err("manifest.artifacts is not sorted by fixture_id".into());
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != manifest.artifacts.len() {
        return Err("manifest.artifacts has duplicate fixture_id values".into());
    }
    Ok(manifest)
}

pub fn verify_manifest_hashes(root: &Path, manifest: &GoldenManifest) -> Result<(), String> {
    for artifact in &manifest.artifacts {
        let rel = format!("{GOLDEN_DIR}/{}", artifact.path);
        let actual = crate::repo::file_sha256_hex(root, &rel)?;
        if actual != artifact.sha256 {
            return Err(format!(
                "{}: manifest hash drifted for {}",
                artifact.fixture_id, artifact.path
            ));
        }
        let tier = artifact.tier()?;
        if tier == EquivalenceTier::Tier1Raw && artifact.canonicalization_fn.is_some() {
            return Err(format!(
                "{}: Tier1Raw must not carry canonicalization_fn",
                artifact.fixture_id
            ));
        }
        if artifact.source.is_empty() {
            return Err(format!("{}: source is empty", artifact.fixture_id));
        }
    }
    Ok(())
}

pub fn verify_checksums_file(root: &Path) -> Result<usize, String> {
    let text = crate::repo::read_text(root, CHECKSUMS_REL)?;
    let mut rows = 0;
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((digest, rel)) = line.split_once("  ") else {
            return Err(format!(
                "checksums.sha256:{} is not `<sha256>  <relpath>`",
                index + 1
            ));
        };
        if digest.len() != 64 || !digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(format!("checksums.sha256:{} has a bad digest", index + 1));
        }
        let rel_path = format!("{GOLDEN_DIR}/{rel}");
        let actual = crate::repo::file_sha256_hex(root, &rel_path)?;
        if actual != digest {
            return Err(format!(
                "{rel} checksum drifted: expected {digest} got {actual}"
            ));
        }
        rows += 1;
    }
    if rows == 0 {
        return Err("checksums.sha256 has no rows".into());
    }
    Ok(rows)
}

pub fn verify_tier1_byte_equality(root: &Path, manifest: &GoldenManifest) -> Result<usize, String> {
    let mut checked = 0;
    for artifact in &manifest.artifacts {
        if artifact.tier()? != EquivalenceTier::Tier1Raw {
            continue;
        }
        let source = artifact.source_artifact_path.as_deref().ok_or_else(|| {
            format!(
                "{}: Tier1Raw requires source_artifact_path",
                artifact.fixture_id
            )
        })?;
        let live = read_bytes(root, source)?;
        let golden = read_bytes(root, &format!("{GOLDEN_DIR}/{}", artifact.path))?;
        if live != golden {
            return Err(format!(
                "{}: Tier1Raw bytes drifted vs {source} (live={} golden={})",
                artifact.fixture_id,
                sha256_hex(&live),
                sha256_hex(&golden)
            ));
        }
        if sha256_hex(&live) != artifact.sha256 {
            return Err(format!(
                "{}: live source hash does not match manifest",
                artifact.fixture_id
            ));
        }
        checked += 1;
    }
    if checked == 0 {
        return Err("no Tier1Raw artifacts in manifest".into());
    }
    Ok(checked)
}

pub fn load_tier3_logical(root: &Path) -> Result<Value, String> {
    let text = crate::repo::read_text(root, TIER3_REL)?;
    serde_json::from_str(&text).map_err(|error| format!("tier3 parse: {error}"))
}

pub fn assert_tier3_invariants(dump: &Value) -> Result<(), String> {
    if dump.get("equivalence_tier").and_then(Value::as_str) != Some("Tier3Logical") {
        return Err("tier3 dump is not labeled Tier3Logical".into());
    }
    let universe = dump
        .get("feature_universe")
        .ok_or("tier3 missing feature_universe")?;
    let feature_count = universe
        .get("feature_count")
        .and_then(Value::as_u64)
        .ok_or("tier3 feature_count")?;
    let declared = universe
        .get("declared_id_count")
        .and_then(Value::as_u64)
        .ok_or("tier3 declared_id_count")?;
    if feature_count != declared {
        return Err(format!(
            "feature_count {feature_count} != declared_id_count {declared}"
        ));
    }
    let weight = universe
        .get("weight_sum")
        .and_then(Value::as_f64)
        .ok_or("tier3 weight_sum")?;
    if (weight - 1.0).abs() > 1e-9 {
        return Err(format!("weight_sum {weight} is not 1.0"));
    }
    let hist = universe
        .get("status_histogram")
        .ok_or("tier3 status_histogram")?;
    let present = hist.get("present").and_then(Value::as_u64).unwrap_or(0);
    let partial = hist.get("partial").and_then(Value::as_u64).unwrap_or(0);
    let missing = hist.get("missing").and_then(Value::as_u64).unwrap_or(0);
    let excluded = hist.get("excluded").and_then(Value::as_u64).unwrap_or(0);
    if present + partial + missing + excluded != feature_count {
        return Err("status histogram does not sum to feature_count".into());
    }
    let keys = universe
        .get("required_row_keys")
        .and_then(Value::as_array)
        .ok_or("tier3 required_row_keys")?;
    for needed in ["id", "family", "name", "status", "weight"] {
        if !keys.iter().any(|key| key.as_str() == Some(needed)) {
            return Err(format!("required_row_keys missing {needed}"));
        }
    }
    let verifiers = dump
        .get("spec_verifiers")
        .ok_or("tier3 missing spec_verifiers")?;
    let wired = verifiers
        .get("wired_count")
        .and_then(Value::as_u64)
        .ok_or("tier3 wired_count")?;
    let catalog = verifiers
        .get("catalog_tag_count")
        .and_then(Value::as_u64)
        .ok_or("tier3 catalog_tag_count")?;
    let unverified = verifiers
        .get("unverified_count")
        .and_then(Value::as_u64)
        .ok_or("tier3 unverified_count")?;
    let live = all_verifiers().len() as u64;
    if wired != live {
        return Err(format!("wired_count {wired} != live verifiers {live}"));
    }
    if catalog != 53 {
        return Err(format!("catalog_tag_count is {catalog}, expected 53"));
    }
    if unverified != catalog.saturating_sub(wired) {
        return Err("unverified_count is not catalog - wired".into());
    }
    let contract = dump.get("contract_md").ok_or("tier3 missing contract_md")?;
    if contract.get("section_count").and_then(Value::as_u64) != Some(8) {
        return Err("CONTRACT.md section_count is not 8".into());
    }
    let schema = dump
        .get("schema_zero_result_v1")
        .ok_or("tier3 missing schema_zero_result_v1")?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or("tier3 schema required")?;
    if required
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        != ["ack", "content"]
    {
        return Err("zero-result-v1 required keys drifted".into());
    }
    let fixture = dump
        .get("fixture_raw_worker_v2")
        .ok_or("tier3 missing fixture_raw_worker_v2")?;
    if fixture.get("entry_count").and_then(Value::as_u64) != Some(4) {
        return Err("raw_worker_v2 entry_count is not 4".into());
    }
    Ok(())
}

pub fn assert_tier2_not_mislabeled(root: &Path, manifest: &GoldenManifest) -> Result<(), String> {
    for artifact in &manifest.artifacts {
        let tier = artifact.tier()?;
        if tier != EquivalenceTier::Tier2Canonical {
            continue;
        }
        if artifact.canonicalization_fn.is_none() {
            return Err(format!(
                "{}: Tier2Canonical must name canonicalization_fn",
                artifact.fixture_id
            ));
        }
        if artifact.tier == EquivalenceTier::Tier1Raw.as_str() {
            return Err(format!(
                "{}: Tier-2 match labeled as Tier-1",
                artifact.fixture_id
            ));
        }
        if artifact.path.ends_with(".json") {
            let rel = format!("{GOLDEN_DIR}/{}", artifact.path);
            let text = crate::repo::read_text(root, &rel)?;
            if text.contains("\"created_at_utc\"") || text.contains("\"run_id\"") {
                return Err(format!(
                    "{}: Tier2 canonical JSON still has a volatile field",
                    artifact.fixture_id
                ));
            }
        }
    }
    Ok(())
}

pub fn verify_all(root: &Path) -> Result<String, String> {
    let manifest = load_manifest(root)?;
    verify_manifest_hashes(root, &manifest)?;
    let checksums = verify_checksums_file(root)?;
    let tier1 = verify_tier1_byte_equality(root, &manifest)?;
    assert_tier2_not_mislabeled(root, &manifest)?;
    let dump = load_tier3_logical(root)?;
    assert_tier3_invariants(&dump)?;
    Ok(format!(
        "artifacts={} checksums={checksums} tier1={tier1} schema={MANIFEST_SCHEMA_VERSION}",
        manifest.artifacts.len()
    ))
}

pub fn verify_repo() -> Result<String, String> {
    verify_all(&repo_root())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_parse_rejects_unknown() {
        assert!(EquivalenceTier::parse("Tier2").is_err());
        assert_eq!(
            EquivalenceTier::parse("Tier2Canonical").unwrap(),
            EquivalenceTier::Tier2Canonical
        );
    }
}
