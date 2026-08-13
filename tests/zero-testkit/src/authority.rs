use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt;

pub const CLAIM_COUNT: usize = 138;
pub const FREEZE_COUNT: usize = 14;
const SOURCE_AUTHORITY_COUNT: usize = 5;
const CLAIM_PROOF_STATE: &str = "NOT_YET_PROVEN";
const FREEZE_PROOF_STATE: &str = "UNIMPLEMENTED";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthorityErrorCode {
    ImportProvenanceGap,
    PristineHashMismatch,
    ClaimCountMismatch,
    FreezeCountMismatch,
    DuplicateClaimId,
    DuplicateFreezeId,
    MissingFreezeId,
    InvalidAuthority,
    MissingReceiptField,
    UnregisteredPlatformProfile,
    UnsupportedPlatformProfile,
    EstimateAsFact,
    ReceiptNotPassing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityError {
    pub code: AuthorityErrorCode,
    pub detail: String,
}

impl AuthorityError {
    fn new(code: AuthorityErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for AuthorityError {}

fn parse_json(bytes: &[u8], name: &str) -> Result<Value, AuthorityError> {
    serde_json::from_slice(bytes).map_err(|error| {
        AuthorityError::new(
            AuthorityErrorCode::InvalidAuthority,
            format!("{name} is not valid JSON: {error}"),
        )
    })
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, AuthorityError> {
    value.get(key).and_then(Value::as_array).ok_or_else(|| {
        AuthorityError::new(
            AuthorityErrorCode::InvalidAuthority,
            format!("missing array {key}"),
        )
    })
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, AuthorityError> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| {
        AuthorityError::new(
            AuthorityErrorCode::InvalidAuthority,
            format!("missing string {key}"),
        )
    })
}

fn source<'a>(provenance: &'a Value, source_id: &str) -> Result<&'a Value, AuthorityError> {
    array(provenance, "source_authorities")?
        .iter()
        .find(|source| source.get("source_id").and_then(Value::as_str) == Some(source_id))
        .ok_or_else(|| {
            AuthorityError::new(
                AuthorityErrorCode::ImportProvenanceGap,
                format!("missing named source authority {source_id}"),
            )
        })
}

fn verify_hash(
    source: &Value,
    field: &str,
    bytes: &[u8],
    label: &str,
) -> Result<(), AuthorityError> {
    let expected = string(source, field)?;
    let actual = zero_abi::sha256_hex(bytes);
    if actual != expected {
        return Err(AuthorityError::new(
            AuthorityErrorCode::PristineHashMismatch,
            format!("{label} SHA-256 mismatch: expected {expected}, got {actual}"),
        ));
    }
    Ok(())
}

fn validate_sources(provenance: &Value) -> Result<(), AuthorityError> {
    let sources = array(provenance, "source_authorities")?;
    if sources.len() != SOURCE_AUTHORITY_COUNT {
        return Err(AuthorityError::new(
            AuthorityErrorCode::ImportProvenanceGap,
            format!(
                "expected {SOURCE_AUTHORITY_COUNT} named source authorities, got {}",
                sources.len()
            ),
        ));
    }
    let expected = BTreeSet::from([
        "round5_archive",
        "round8_freezes",
        "round8_plan",
        "round8_convergence_receipt",
        "round8_validation_receipt",
    ]);
    let actual: BTreeSet<_> = sources
        .iter()
        .filter_map(|value| value.get("source_id").and_then(Value::as_str))
        .collect();
    if actual != expected {
        return Err(AuthorityError::new(
            AuthorityErrorCode::ImportProvenanceGap,
            "named source authority set differs from the frozen five-input set",
        ));
    }
    Ok(())
}

fn claim_rows(claim_ledger: &Value) -> Result<Vec<Value>, AuthorityError> {
    let claims = array(claim_ledger, "claims")?;
    let declared = claim_ledger
        .get("claim_count")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    if declared != CLAIM_COUNT || claims.len() != CLAIM_COUNT {
        return Err(AuthorityError::new(
            AuthorityErrorCode::ClaimCountMismatch,
            format!(
                "expected {CLAIM_COUNT} claims, declared {declared}, observed {}",
                claims.len()
            ),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut rows = Vec::with_capacity(claims.len());
    for (index, raw) in claims.iter().enumerate() {
        let claim_id = string(raw, "claim_id")?;
        if !ids.insert(claim_id.to_owned()) {
            return Err(AuthorityError::new(
                AuthorityErrorCode::DuplicateClaimId,
                format!("duplicate claim id {claim_id}"),
            ));
        }
        rows.push(json!({
            "claim_id": claim_id,
            "proof_state": CLAIM_PROOF_STATE,
            "raw_sha256": zero_abi::sha256_hex(zero_abi::canonical_json(raw).as_bytes()),
            "source": {
                "source_id": "round5_archive",
                "member": "ZeroStack-RACC-V3-Round5-Maximal-Certified-Zero-Kernel-Package/02_CLAIM_LEDGER.json",
                "ordinal": index + 1
            },
            "raw": raw
        }));
    }
    Ok(rows)
}

fn freeze_rows(freeze_source: &Value) -> Result<Vec<Value>, AuthorityError> {
    let freezes = array(freeze_source, "freezes")?;
    let declared = freeze_source
        .get("executable_freeze_count")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    if declared != FREEZE_COUNT || freezes.len() != FREEZE_COUNT {
        return Err(AuthorityError::new(
            AuthorityErrorCode::FreezeCountMismatch,
            format!(
                "expected {FREEZE_COUNT} freezes, declared {declared}, observed {}",
                freezes.len()
            ),
        ));
    }
    let required: BTreeSet<String> = [
        "Z0", "Z1", "Z2", "Z3", "Z4", "Z5", "Z6", "Z7", "Z8", "E-FS", "E-GRAPH", "E-TOKEN", "P1",
        "S1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut ids = BTreeSet::new();
    let mut rows = Vec::with_capacity(freezes.len());
    for (index, raw) in freezes.iter().enumerate() {
        let freeze_id = string(raw, "id")?;
        if !ids.insert(freeze_id.to_owned()) {
            return Err(AuthorityError::new(
                AuthorityErrorCode::DuplicateFreezeId,
                format!("duplicate freeze id {freeze_id}"),
            ));
        }
        rows.push(json!({
            "freeze_id": freeze_id,
            "proof_state": FREEZE_PROOF_STATE,
            "raw_sha256": zero_abi::sha256_hex(zero_abi::canonical_json(raw).as_bytes()),
            "source": {
                "source_id": "round8_freezes",
                "ordinal": index + 1
            },
            "raw": raw
        }));
    }
    if ids != required {
        let missing: Vec<_> = required.difference(&ids).cloned().collect();
        return Err(AuthorityError::new(
            AuthorityErrorCode::MissingFreezeId,
            format!("freeze id set differs from authority; missing {missing:?}"),
        ));
    }
    Ok(rows)
}

pub fn generate_authority(
    claim_ledger_bytes: &[u8],
    freeze_bytes: &[u8],
    source_audit_bytes: &[u8],
    provenance_bytes: &[u8],
) -> Result<Vec<u8>, AuthorityError> {
    let claim_ledger = parse_json(claim_ledger_bytes, "claim ledger")?;
    let freeze_source = parse_json(freeze_bytes, "freeze source")?;
    let source_audit = parse_json(source_audit_bytes, "source archive audit")?;
    let provenance = parse_json(provenance_bytes, "provenance")?;
    validate_sources(&provenance)?;

    let round5 = source(&provenance, "round5_archive")?;
    verify_hash(
        round5,
        "claim_ledger_sha256",
        claim_ledger_bytes,
        "claim ledger",
    )?;
    verify_hash(
        round5,
        "source_archive_audit_sha256",
        source_audit_bytes,
        "source archive audit",
    )?;
    verify_hash(
        source(&provenance, "round8_freezes")?,
        "sha256",
        freeze_bytes,
        "freeze source",
    )?;

    let claims = claim_rows(&claim_ledger)?;
    let freezes = freeze_rows(&freeze_source)?;
    let authority = json!({
        "schema": "zerostack.canonical-claim-authority.v1",
        "authority_version": provenance.get("authority_version"),
        "authority_hash": provenance.get("authority_hash"),
        "authority_hash_algorithm": provenance.get("authority_hash_algorithm"),
        "claim_count": claims.len(),
        "freeze_count": freezes.len(),
        "claims": claims,
        "freezes": freezes,
        "source_archive_audit": source_audit,
        "provenance": provenance
    });
    let mut bytes = zero_abi::canonical_json(&authority).into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn validate_passing_receipt(
    authority_bytes: &[u8],
    receipt_bytes: &[u8],
) -> Result<(), AuthorityError> {
    let authority = parse_json(authority_bytes, "canonical authority")?;
    let receipt = parse_json(receipt_bytes, "proof receipt")?;
    let provenance = authority.get("provenance").ok_or_else(|| {
        AuthorityError::new(
            AuthorityErrorCode::InvalidAuthority,
            "canonical authority lacks provenance",
        )
    })?;
    for field in array(provenance, "proof_receipt_required_fields")? {
        let field = field.as_str().ok_or_else(|| {
            AuthorityError::new(
                AuthorityErrorCode::InvalidAuthority,
                "receipt field name is not a string",
            )
        })?;
        if receipt.get(field).is_none() {
            return Err(AuthorityError::new(
                AuthorityErrorCode::MissingReceiptField,
                format!("proof receipt lacks {field}"),
            ));
        }
    }
    if receipt.get("result").and_then(Value::as_str) != Some("PASS") {
        return Err(AuthorityError::new(
            AuthorityErrorCode::ReceiptNotPassing,
            "only a PASS receipt can be validated as passing",
        ));
    }
    if matches!(
        receipt.get("evidence_kind").and_then(Value::as_str),
        Some("estimate" | "declared_estimate")
    ) {
        return Err(AuthorityError::new(
            AuthorityErrorCode::EstimateAsFact,
            "an estimate cannot be presented as measured passing evidence",
        ));
    }
    let profile_id = string(&receipt, "platform_profile")?;
    let profiles = provenance
        .pointer("/preregistration/target_profiles")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AuthorityError::new(
                AuthorityErrorCode::InvalidAuthority,
                "authority lacks target profile preregistration",
            )
        })?;
    let profile = profiles
        .iter()
        .find(|profile| profile.get("profile_id").and_then(Value::as_str) == Some(profile_id))
        .ok_or_else(|| {
            AuthorityError::new(
                AuthorityErrorCode::UnregisteredPlatformProfile,
                format!("platform profile {profile_id} is not preregistered"),
            )
        })?;
    if receipt.get("native_claim").and_then(Value::as_bool) == Some(true)
        && profile
            .get("passing_native_receipt_allowed")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(AuthorityError::new(
            AuthorityErrorCode::UnsupportedPlatformProfile,
            format!("platform profile {profile_id} lacks a passing native receipt"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAIMS: &[u8] =
        include_bytes!("../../../conformance/authority/sources/round5-claim-ledger.json");
    const FREEZES: &[u8] =
        include_bytes!("../../../conformance/authority/sources/round8-executable-freezes.json");
    const AUDIT: &[u8] =
        include_bytes!("../../../conformance/authority/sources/round5-source-archive-audit.json");
    const PROVENANCE: &[u8] = include_bytes!("../../../conformance/authority/provenance-v1.json");
    const CANONICAL: &[u8] =
        include_bytes!("../../../conformance/authority/canonical-authority-v1.json");

    fn provenance_with_hash(field: &str, bytes: &[u8]) -> Vec<u8> {
        let mut provenance: Value = serde_json::from_slice(PROVENANCE).unwrap();
        provenance["source_authorities"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|source| {
                source["source_id"]
                    == if field == "claim_ledger_sha256" {
                        "round5_archive"
                    } else {
                        "round8_freezes"
                    }
            })
            .unwrap()[field] = json!(zero_abi::sha256_hex(bytes));
        serde_json::to_vec(&provenance).unwrap()
    }

    fn passing_receipt() -> Value {
        json!({
            "schema_version": 1,
            "bead_key": "Z0",
            "claim_or_freeze_ids": ["Z0"],
            "assembly_manifest_digest": "0".repeat(64),
            "source_repository_heads": {"ZeroStack": "0".repeat(40)},
            "model_or_spec_version": "authority-v1",
            "toolchain_identities": ["rust-test"],
            "exact_commands": ["cargo test -p zero-testkit authority_ledger"],
            "input_fixture_hashes": {},
            "output_artifact_hashes": {},
            "mutants_run": [],
            "platform_profile": "darwin-aarch64",
            "result": "PASS",
            "failure_code": null,
            "residual_assumptions": [],
            "started_at": "2026-08-04T00:00:00Z",
            "completed_at": "2026-08-04T00:00:01Z",
            "evidence_kind": "measured",
            "native_claim": false
        })
    }

    #[test]
    fn authority_ledger_counts_and_preserves_raw_rows() {
        let generated = generate_authority(CLAIMS, FREEZES, AUDIT, PROVENANCE).unwrap();
        let authority: Value = serde_json::from_slice(&generated).unwrap();
        let source_claims: Value = serde_json::from_slice(CLAIMS).unwrap();
        let source_freezes: Value = serde_json::from_slice(FREEZES).unwrap();
        assert_eq!(authority["claim_count"], CLAIM_COUNT);
        assert_eq!(authority["freeze_count"], FREEZE_COUNT);
        let generated_claims: Vec<_> = authority["claims"]
            .as_array()
            .unwrap()
            .iter()
            .map(|claim| claim["raw"].clone())
            .collect();
        let generated_freezes: Vec<_> = authority["freezes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|freeze| freeze["raw"].clone())
            .collect();
        assert_eq!(
            generated_claims.as_slice(),
            source_claims["claims"].as_array().unwrap().as_slice()
        );
        assert_eq!(
            generated_freezes.as_slice(),
            source_freezes["freezes"].as_array().unwrap().as_slice()
        );
        assert!(
            authority["claims"]
                .as_array()
                .unwrap()
                .iter()
                .all(|claim| claim["proof_state"] == CLAIM_PROOF_STATE)
        );
        assert!(
            authority["freezes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|freeze| freeze["proof_state"] == FREEZE_PROOF_STATE)
        );
    }

    #[test]
    fn provenance_round_trip_is_byte_identical() {
        let first = generate_authority(CLAIMS, FREEZES, AUDIT, PROVENANCE).unwrap();
        let second = generate_authority(CLAIMS, FREEZES, AUDIT, PROVENANCE).unwrap();
        assert_eq!(first, second);
        assert_eq!(first, CANONICAL);
    }

    #[test]
    fn authority_ledger_rejects_duplicate_claim_and_missing_freeze_mutants() {
        let mut claims: Value = serde_json::from_slice(CLAIMS).unwrap();
        claims["claims"][1]["claim_id"] = claims["claims"][0]["claim_id"].clone();
        let claims = serde_json::to_vec(&claims).unwrap();
        let provenance = provenance_with_hash("claim_ledger_sha256", &claims);
        assert_eq!(
            generate_authority(&claims, FREEZES, AUDIT, &provenance)
                .unwrap_err()
                .code,
            AuthorityErrorCode::DuplicateClaimId
        );

        let mut freezes: Value = serde_json::from_slice(FREEZES).unwrap();
        freezes["freezes"].as_array_mut().unwrap().pop();
        freezes["executable_freeze_count"] = json!(13);
        let freezes = serde_json::to_vec(&freezes).unwrap();
        let provenance = provenance_with_hash("sha256", &freezes);
        assert_eq!(
            generate_authority(CLAIMS, &freezes, AUDIT, &provenance)
                .unwrap_err()
                .code,
            AuthorityErrorCode::FreezeCountMismatch
        );

        let mut freezes: Value = serde_json::from_slice(FREEZES).unwrap();
        freezes["freezes"][1]["id"] = freezes["freezes"][0]["id"].clone();
        let freezes = serde_json::to_vec(&freezes).unwrap();
        let provenance = provenance_with_hash("sha256", &freezes);
        assert_eq!(
            generate_authority(CLAIMS, &freezes, AUDIT, &provenance)
                .unwrap_err()
                .code,
            AuthorityErrorCode::DuplicateFreezeId
        );

        let mut freezes: Value = serde_json::from_slice(FREEZES).unwrap();
        freezes["freezes"][0]["id"] = json!("Z0-REMOVED");
        let freezes = serde_json::to_vec(&freezes).unwrap();
        let provenance = provenance_with_hash("sha256", &freezes);
        assert_eq!(
            generate_authority(CLAIMS, &freezes, AUDIT, &provenance)
                .unwrap_err()
                .code,
            AuthorityErrorCode::MissingFreezeId
        );
    }

    #[test]
    fn provenance_round_trip_rejects_missing_source_and_pristine_hash_mutants() {
        let mut provenance: Value = serde_json::from_slice(PROVENANCE).unwrap();
        provenance["source_authorities"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(
            generate_authority(
                CLAIMS,
                FREEZES,
                AUDIT,
                &serde_json::to_vec(&provenance).unwrap(),
            )
            .unwrap_err()
            .code,
            AuthorityErrorCode::ImportProvenanceGap
        );
        let mut mutated = CLAIMS.to_vec();
        mutated.push(b' ');
        assert_eq!(
            generate_authority(&mutated, FREEZES, AUDIT, PROVENANCE)
                .unwrap_err()
                .code,
            AuthorityErrorCode::PristineHashMismatch
        );
    }

    #[test]
    fn authority_ledger_preserves_duplicate_digest_observations() {
        let authority: Value = serde_json::from_slice(CANONICAL).unwrap();
        let observations = authority
            .pointer("/provenance/manifest_lineage_observations")
            .and_then(Value::as_array)
            .unwrap();
        let plan_digest = "13b2b76aeb6924da8cfa02a247221130d6524b6da165abc84fe6e6307b820db5";
        let matching: Vec<_> = observations
            .iter()
            .filter(|row| row["observed_sha256"] == plan_digest)
            .collect();
        assert_eq!(matching.len(), 2);
        assert_ne!(matching[0]["record_id"], matching[1]["record_id"]);
        assert_ne!(
            matching[0]["source_coordinate"],
            matching[1]["source_coordinate"]
        );
    }

    #[test]
    fn authority_ledger_receipt_rejects_unregistered_target_native_gap_and_estimate() {
        let mut receipt = passing_receipt();
        receipt.as_object_mut().unwrap().remove("exact_commands");
        assert_eq!(
            validate_passing_receipt(CANONICAL, &serde_json::to_vec(&receipt).unwrap())
                .unwrap_err()
                .code,
            AuthorityErrorCode::MissingReceiptField
        );

        let mut receipt = passing_receipt();
        receipt["platform_profile"] = json!("unregistered-target");
        assert_eq!(
            validate_passing_receipt(CANONICAL, &serde_json::to_vec(&receipt).unwrap())
                .unwrap_err()
                .code,
            AuthorityErrorCode::UnregisteredPlatformProfile
        );

        let mut receipt = passing_receipt();
        receipt["native_claim"] = json!(true);
        assert_eq!(
            validate_passing_receipt(CANONICAL, &serde_json::to_vec(&receipt).unwrap())
                .unwrap_err()
                .code,
            AuthorityErrorCode::UnsupportedPlatformProfile
        );

        let mut receipt = passing_receipt();
        receipt["evidence_kind"] = json!("estimate");
        assert_eq!(
            validate_passing_receipt(CANONICAL, &serde_json::to_vec(&receipt).unwrap())
                .unwrap_err()
                .code,
            AuthorityErrorCode::EstimateAsFact
        );
    }
}
