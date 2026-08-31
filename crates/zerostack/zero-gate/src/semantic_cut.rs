//! Proof-carrying exact RCQ and reasoning-epoch contraction contract. The trusted path supports only
//! exact continuation identity. A clean restart, approximate continuation, or classifier score
//! cannot mint this capability; those paths require a quality guard and frozen-baseline deoptimization.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{canonical_json, sha256};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};

pub type SemanticCutDigest = [u8; 32];

pub const SEMANTIC_CUT_CONTRACT_VERSION: u16 = 1;
pub const SEMANTIC_CUT_SCHEMA_VERSION: &str = "racc-r-semantic-cut";
pub const SEMANTIC_CUT_MAX_CANONICAL_BYTES: usize = 64 * 1024;

const CLAIM_DOMAIN: &[u8] = b"zerostack.semantic_cut.claim\0";
const CERTIFICATE_DOMAIN: &[u8] = b"zerostack.semantic_cut.certificate\0";
const CONTRACT_DOMAIN: &[u8] = b"zerostack.semantic_cut.contract\0";
const RCQ_DOMAIN: &[u8] = b"zerostack.semantic_cut.rcq_identity\0";
const PROJECT_RELATION_DOMAIN: &[u8] = b"zerostack.semantic_cut.project_relation\0";
const EFFECT_RELATION_DOMAIN: &[u8] = b"zerostack.semantic_cut.effect_relation\0";
const VERIFIER_DOMAIN: &[u8] = b"zerostack.semantic_cut.verifier_identity\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStateStatus {
    ExactPreserved,
    ExactCleanRestart,
    ScopedEquivalent,
    Approximate,
    Unavailable,
    Expired,
    IdentityMismatch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticAuthority {
    DeterministicOnly,
    TaskSemanticSelection,
}

/// Typed epoch boundary. This is a state record, not proof by itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningSafepoint {
    schema_version: String,
    project_control_root: SemanticCutDigest,
    world_fiber_digest: SemanticCutDigest,
    evidence_state_digest: SemanticCutDigest,
    reasoning_contract_digest: SemanticCutDigest,
    fixed_model_digest: SemanticCutDigest,
    opaque_reasoning_state_digest: SemanticCutDigest,
    reasoning_state_status: ReasoningStateStatus,
    open_obligations_digest: SemanticCutDigest,
    resource_risk_reserve_digest: SemanticCutDigest,
    baseline_replay_cursor_digest: SemanticCutDigest,
    transaction_root_digest: SemanticCutDigest,
    receipt_head_digest: SemanticCutDigest,
}

impl ReasoningSafepoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_control_root: SemanticCutDigest,
        world_fiber_digest: SemanticCutDigest,
        evidence_state_digest: SemanticCutDigest,
        reasoning_contract_digest: SemanticCutDigest,
        fixed_model_digest: SemanticCutDigest,
        opaque_reasoning_state_digest: SemanticCutDigest,
        reasoning_state_status: ReasoningStateStatus,
        open_obligations_digest: SemanticCutDigest,
        resource_risk_reserve_digest: SemanticCutDigest,
        baseline_replay_cursor_digest: SemanticCutDigest,
        transaction_root_digest: SemanticCutDigest,
        receipt_head_digest: SemanticCutDigest,
    ) -> Result<Self, SemanticCutError> {
        let value = Self {
            schema_version: SEMANTIC_CUT_SCHEMA_VERSION.into(),
            project_control_root,
            world_fiber_digest,
            evidence_state_digest,
            reasoning_contract_digest,
            fixed_model_digest,
            opaque_reasoning_state_digest,
            reasoning_state_status,
            open_obligations_digest,
            resource_risk_reserve_digest,
            baseline_replay_cursor_digest,
            transaction_root_digest,
            receipt_head_digest,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SemanticCutError> {
        if self.schema_version != SEMANTIC_CUT_SCHEMA_VERSION {
            return Err(cut_error(
                SemanticCutFailureCode::SchemaVersionMismatch,
                "reasoning safepoint schema is unsupported",
            ));
        }
        for (label, digest) in [
            ("project/control root", self.project_control_root),
            ("world fiber", self.world_fiber_digest),
            ("evidence state", self.evidence_state_digest),
            ("reasoning contract", self.reasoning_contract_digest),
            ("fixed model", self.fixed_model_digest),
            ("opaque reasoning state", self.opaque_reasoning_state_digest),
            ("open obligations", self.open_obligations_digest),
            ("resource/risk reserve", self.resource_risk_reserve_digest),
            ("baseline replay cursor", self.baseline_replay_cursor_digest),
            ("transaction root", self.transaction_root_digest),
            ("receipt head", self.receipt_head_digest),
        ] {
            require_nonzero(label, digest)?;
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SemanticCutError> {
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SemanticCutError> {
        let safepoint: Self = decode_canonical(bytes)?;
        safepoint.validate()?;
        Ok(safepoint)
    }

    pub fn digest(&self) -> Result<SemanticCutDigest, SemanticCutError> {
        digest_serialized(b"zerostack.semantic_cut.safepoint\0", self)
    }

    pub const fn project_control_root(&self) -> SemanticCutDigest {
        self.project_control_root
    }
    pub const fn reasoning_contract_digest(&self) -> SemanticCutDigest {
        self.reasoning_contract_digest
    }
    pub const fn fixed_model_digest(&self) -> SemanticCutDigest {
        self.fixed_model_digest
    }
    pub const fn opaque_reasoning_state_digest(&self) -> SemanticCutDigest {
        self.opaque_reasoning_state_digest
    }
    pub const fn reasoning_state_status(&self) -> ReasoningStateStatus {
        self.reasoning_state_status
    }
    pub const fn receipt_head_digest(&self) -> SemanticCutDigest {
        self.receipt_head_digest
    }

    fn exact_continuation_equal(&self, other: &Self) -> bool {
        self.project_control_root == other.project_control_root
            && self.world_fiber_digest == other.world_fiber_digest
            && self.evidence_state_digest == other.evidence_state_digest
            && self.reasoning_contract_digest == other.reasoning_contract_digest
            && self.fixed_model_digest == other.fixed_model_digest
            && self.opaque_reasoning_state_digest == other.opaque_reasoning_state_digest
            && self.open_obligations_digest == other.open_obligations_digest
            && self.resource_risk_reserve_digest == other.resource_risk_reserve_digest
            && self.baseline_replay_cursor_digest == other.baseline_replay_cursor_digest
            && self.transaction_root_digest == other.transaction_root_digest
    }
}

/// Full machine-checkable claim emitted by an exact semantic-cut verifier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCutClaim {
    schema_version: String,
    input_project_control_root: SemanticCutDigest,
    baseline_epoch_class_digest: SemanticCutDigest,
    compiled_plan_digest: SemanticCutDigest,
    semantic_authority: SemanticAuthority,
    baseline_terminal: ReasoningSafepoint,
    compiled_terminal: ReasoningSafepoint,
    baseline_external_effects_digest: SemanticCutDigest,
    compiled_external_effects_digest: SemanticCutDigest,
    baseline_attribution_identity_digest: SemanticCutDigest,
    compiled_attribution_identity_digest: SemanticCutDigest,
    baseline_resource_receipt_digest: SemanticCutDigest,
    compiled_resource_receipt_digest: SemanticCutDigest,
    comparison_identity_digest: SemanticCutDigest,
    certificate_scope_digest: SemanticCutDigest,
    deoptimization_map_digest: SemanticCutDigest,
}

impl SemanticCutClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new_exact(
        input_project_control_root: SemanticCutDigest,
        baseline_epoch_class_digest: SemanticCutDigest,
        compiled_plan_digest: SemanticCutDigest,
        baseline_terminal: ReasoningSafepoint,
        compiled_terminal: ReasoningSafepoint,
        baseline_external_effects_digest: SemanticCutDigest,
        compiled_external_effects_digest: SemanticCutDigest,
        baseline_attribution_identity_digest: SemanticCutDigest,
        compiled_attribution_identity_digest: SemanticCutDigest,
        baseline_resource_receipt_digest: SemanticCutDigest,
        compiled_resource_receipt_digest: SemanticCutDigest,
        comparison_identity_digest: SemanticCutDigest,
        certificate_scope_digest: SemanticCutDigest,
        deoptimization_map_digest: SemanticCutDigest,
    ) -> Result<Self, SemanticCutError> {
        let claim = Self {
            schema_version: SEMANTIC_CUT_SCHEMA_VERSION.into(),
            input_project_control_root,
            baseline_epoch_class_digest,
            compiled_plan_digest,
            semantic_authority: SemanticAuthority::DeterministicOnly,
            baseline_terminal,
            compiled_terminal,
            baseline_external_effects_digest,
            compiled_external_effects_digest,
            baseline_attribution_identity_digest,
            compiled_attribution_identity_digest,
            baseline_resource_receipt_digest,
            compiled_resource_receipt_digest,
            comparison_identity_digest,
            certificate_scope_digest,
            deoptimization_map_digest,
        };
        claim.validate_exact()?;
        Ok(claim)
    }

    pub fn validate_exact(&self) -> Result<(), SemanticCutError> {
        if self.schema_version != SEMANTIC_CUT_SCHEMA_VERSION {
            return Err(cut_error(
                SemanticCutFailureCode::SchemaVersionMismatch,
                "semantic-cut claim schema is unsupported",
            ));
        }
        self.baseline_terminal.validate()?;
        self.compiled_terminal.validate()?;
        for (label, digest) in [
            (
                "input project/control root",
                self.input_project_control_root,
            ),
            ("baseline epoch class", self.baseline_epoch_class_digest),
            ("compiled plan", self.compiled_plan_digest),
            (
                "baseline external effects",
                self.baseline_external_effects_digest,
            ),
            (
                "compiled external effects",
                self.compiled_external_effects_digest,
            ),
            (
                "baseline attribution identity",
                self.baseline_attribution_identity_digest,
            ),
            (
                "compiled attribution identity",
                self.compiled_attribution_identity_digest,
            ),
            (
                "baseline resource receipt",
                self.baseline_resource_receipt_digest,
            ),
            (
                "compiled resource receipt",
                self.compiled_resource_receipt_digest,
            ),
            ("comparison identity", self.comparison_identity_digest),
            ("certificate scope", self.certificate_scope_digest),
            ("deoptimization map", self.deoptimization_map_digest),
        ] {
            require_nonzero(label, digest)?;
        }
        if self.semantic_authority != SemanticAuthority::DeterministicOnly {
            return Err(cut_error(
                SemanticCutFailureCode::SemanticAuthorityCrossing,
                "semantic-cut plan may contain only declared deterministic operations",
            ));
        }
        if self.baseline_terminal.reasoning_state_status != ReasoningStateStatus::ExactPreserved
            || self.compiled_terminal.reasoning_state_status != ReasoningStateStatus::ExactPreserved
        {
            return Err(cut_error(
                SemanticCutFailureCode::ContinuationNotExact,
                "strict epoch contraction requires exact preserved reasoning state",
            ));
        }
        if !self
            .baseline_terminal
            .exact_continuation_equal(&self.compiled_terminal)
        {
            return Err(cut_error(
                SemanticCutFailureCode::TerminalStateMismatch,
                "baseline and compiled terminal protected state are not identical",
            ));
        }
        if self.baseline_external_effects_digest != self.compiled_external_effects_digest {
            return Err(cut_error(
                SemanticCutFailureCode::ExternalEffectMismatch,
                "baseline and compiled external-effect identities differ",
            ));
        }
        if self.baseline_attribution_identity_digest != self.compiled_attribution_identity_digest {
            return Err(cut_error(
                SemanticCutFailureCode::AttributionMismatch,
                "compiled transition changes semantic attribution",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SemanticCutError> {
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SemanticCutError> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate_exact()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<SemanticCutDigest, SemanticCutError> {
        digest_serialized(CLAIM_DOMAIN, self)
    }

    pub fn terminal_rcq_identity_digest(&self) -> SemanticCutDigest {
        let terminal = &self.compiled_terminal;
        digest_parts(
            RCQ_DOMAIN,
            &[
                &terminal.fixed_model_digest,
                &terminal.reasoning_contract_digest,
                &terminal.opaque_reasoning_state_digest,
            ],
        )
    }

    pub fn terminal_project_relation_digest(&self) -> SemanticCutDigest {
        digest_parts(
            PROJECT_RELATION_DOMAIN,
            &[
                &self.baseline_terminal.project_control_root,
                &self.compiled_terminal.project_control_root,
            ],
        )
    }

    pub fn external_effect_relation_digest(&self) -> SemanticCutDigest {
        digest_parts(
            EFFECT_RELATION_DOMAIN,
            &[
                &self.baseline_external_effects_digest,
                &self.compiled_external_effects_digest,
            ],
        )
    }

    pub const fn input_project_control_root(&self) -> SemanticCutDigest {
        self.input_project_control_root
    }
    pub const fn compiled_plan_digest(&self) -> SemanticCutDigest {
        self.compiled_plan_digest
    }
    pub const fn comparison_identity_digest(&self) -> SemanticCutDigest {
        self.comparison_identity_digest
    }
    pub const fn certificate_scope_digest(&self) -> SemanticCutDigest {
        self.certificate_scope_digest
    }
    pub const fn reasoning_contract_digest(&self) -> SemanticCutDigest {
        self.compiled_terminal.reasoning_contract_digest
    }
    pub const fn fixed_model_digest(&self) -> SemanticCutDigest {
        self.compiled_terminal.fixed_model_digest
    }
    pub const fn attribution_identity_digest(&self) -> SemanticCutDigest {
        self.compiled_attribution_identity_digest
    }
    pub const fn deoptimization_map_digest(&self) -> SemanticCutDigest {
        self.deoptimization_map_digest
    }
}

/// Opaque G3 authority. Only exact claim validation plus verified bytes can mint it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCutEvidence {
    contract_version: u16,
    claim: SemanticCutClaim,
    claim_digest: SemanticCutDigest,
    evidence_digest: SemanticCutDigest,
    verifier_identity_digest: SemanticCutDigest,
    certificate_digest: SemanticCutDigest,
}

impl SemanticCutEvidence {
    pub fn verify_owner_scoped(
        claim: SemanticCutClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, SemanticCutError> {
        claim.validate_exact()?;
        match (evidence.query(), &evidence.certificate().completeness) {
            (
                Query::BuildReceipt { .. },
                CompletenessWitness::BuildReceipt { exit_code: 0, .. },
            )
            | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
            _ => {
                return Err(cut_error(
                    SemanticCutFailureCode::UnsupportedEvidenceClass,
                    "semantic-cut authority requires a successful verified build or test trace",
                ));
            }
        }
        let canonical_claim = claim.canonical_bytes()?;
        if evidence.payload() != canonical_claim {
            return Err(cut_error(
                SemanticCutFailureCode::EvidencePayloadMismatch,
                "verified evidence payload is not the exact canonical semantic-cut claim",
            ));
        }
        let claim_digest = claim.digest()?;
        let evidence_digest = evidence
            .certificate()
            .canonical_digest()
            .map_err(|error| json_error(error.to_string()))?;
        let verifier_identity_digest = semantic_cut_verifier_identity(evidence);
        let certificate_digest = certificate_digest(
            SEMANTIC_CUT_CONTRACT_VERSION,
            claim_digest,
            evidence_digest,
            verifier_identity_digest,
            claim.terminal_rcq_identity_digest(),
        );
        Ok(Self {
            contract_version: SEMANTIC_CUT_CONTRACT_VERSION,
            claim,
            claim_digest,
            evidence_digest,
            verifier_identity_digest,
            certificate_digest,
        })
    }

    pub fn validate(&self) -> Result<(), SemanticCutError> {
        if self.contract_version != SEMANTIC_CUT_CONTRACT_VERSION {
            return Err(cut_error(
                SemanticCutFailureCode::SchemaVersionMismatch,
                "semantic-cut certificate contract is unsupported",
            ));
        }
        self.claim.validate_exact()?;
        require_nonzero("evidence", self.evidence_digest)?;
        require_nonzero("verifier identity", self.verifier_identity_digest)?;
        if self.claim.digest()? != self.claim_digest
            || certificate_digest(
                self.contract_version,
                self.claim_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
                self.claim.terminal_rcq_identity_digest(),
            ) != self.certificate_digest
        {
            return Err(cut_error(
                SemanticCutFailureCode::CertificateDigestMismatch,
                "semantic-cut certificate digest does not bind its claim and evidence",
            ));
        }
        Ok(())
    }

    pub fn record(&self) -> SemanticCutCertificateRecord {
        SemanticCutCertificateRecord {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
    }

    pub const fn claim(&self) -> &SemanticCutClaim {
        &self.claim
    }
    pub const fn certificate_digest(&self) -> SemanticCutDigest {
        self.certificate_digest
    }
    pub const fn verifier_identity_digest(&self) -> SemanticCutDigest {
        self.verifier_identity_digest
    }
    pub fn terminal_rcq_identity_digest(&self) -> SemanticCutDigest {
        self.claim.terminal_rcq_identity_digest()
    }
}

/// Public receipt form. It is replay-validatable but cannot authorize execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCutCertificateRecord {
    pub contract_version: u16,
    pub claim: SemanticCutClaim,
    pub claim_digest: SemanticCutDigest,
    pub evidence_digest: SemanticCutDigest,
    pub verifier_identity_digest: SemanticCutDigest,
    pub certificate_digest: SemanticCutDigest,
}

impl SemanticCutCertificateRecord {
    pub fn validate(&self) -> Result<(), SemanticCutError> {
        let certificate = SemanticCutEvidence {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        };
        certificate.validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SemanticCutError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SemanticCutError> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

pub fn semantic_cut_verifier_identity(evidence: &VerifiedEvidence<'_, '_>) -> SemanticCutDigest {
    let provenance = evidence.provenance();
    let body = json!({
        "index_id": provenance.index_id,
        "index_version": provenance.index_version,
        "operator_id": provenance.operator_id,
        "operator_version": provenance.operator_version,
        "parser_id": provenance.parser_id,
        "parser_version": provenance.parser_version,
    });
    digest_value(VERIFIER_DOMAIN, &body)
}

pub fn semantic_cut_contract_manifest() -> Value {
    json!({
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "certificate_fields": [
            "contract_version",
            "claim",
            "claim_digest",
            "evidence_digest",
            "verifier_identity_digest",
            "certificate_digest",
        ],
        "claim_fields": [
            "schema_version",
            "input_project_control_root",
            "baseline_epoch_class_digest",
            "compiled_plan_digest",
            "semantic_authority",
            "baseline_terminal",
            "compiled_terminal",
            "baseline_external_effects_digest",
            "compiled_external_effects_digest",
            "baseline_attribution_identity_digest",
            "compiled_attribution_identity_digest",
            "baseline_resource_receipt_digest",
            "compiled_resource_receipt_digest",
            "comparison_identity_digest",
            "certificate_scope_digest",
            "deoptimization_map_digest",
        ],
        "contract_version": SEMANTIC_CUT_CONTRACT_VERSION,
        "epoch_contractible_when": [
            "project_control_state_exact",
            "reasoning_contract_exact",
            "opaque_reasoning_state_exact",
            "external_effects_exact",
            "attribution_exact",
            "resource_receipts_bound",
            "successful_verifier_receipt",
            "verified_claim_payload_exact",
        ],
        "name": "zerostack.semantic_cut",
        "negative_space": [
            "approximate_continuation_as_exact",
            "classifier_score_as_rcq_proof",
            "clean_restart_as_exact_continuation",
            "hidden_task_semantic_selection",
            "reasoning_summary_as_opaque_state",
            "token_reduction_as_quality",
            "unbounded_autonomous_plan",
        ],
        "reasoning_state_statuses": [
            "exact_preserved",
            "exact_clean_restart",
            "scoped_equivalent",
            "approximate",
            "unavailable",
            "expired",
            "identity_mismatch",
        ],
        "safepoint_fields": [
            "schema_version",
            "project_control_root",
            "world_fiber_digest",
            "evidence_state_digest",
            "reasoning_contract_digest",
            "fixed_model_digest",
            "opaque_reasoning_state_digest",
            "reasoning_state_status",
            "open_obligations_digest",
            "resource_risk_reserve_digest",
            "baseline_replay_cursor_digest",
            "transaction_root_digest",
            "receipt_head_digest",
        ],
        "schema_version": SEMANTIC_CUT_SCHEMA_VERSION,
        "strict_proof_mode": "exact_continuation_identity_only",
    })
}

pub fn semantic_cut_contract_digest() -> SemanticCutDigest {
    digest_value(CONTRACT_DOMAIN, &semantic_cut_contract_manifest())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticCutFailureCode {
    SchemaVersionMismatch,
    MissingBinding,
    SemanticAuthorityCrossing,
    ContinuationNotExact,
    TerminalStateMismatch,
    ExternalEffectMismatch,
    AttributionMismatch,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    EvidencePayloadMismatch,
    UnsupportedEvidenceClass,
    CertificateDigestMismatch,
    SerializationFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticCutError {
    failure_code: SemanticCutFailureCode,
    message: String,
}

impl SemanticCutError {
    pub const fn failure_code(&self) -> SemanticCutFailureCode {
        self.failure_code
    }
}

impl fmt::Display for SemanticCutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.failure_code, self.message)
    }
}
impl Error for SemanticCutError {}

fn cut_error(failure_code: SemanticCutFailureCode, message: impl Into<String>) -> SemanticCutError {
    SemanticCutError {
        failure_code,
        message: message.into(),
    }
}

fn json_error(message: impl Into<String>) -> SemanticCutError {
    cut_error(SemanticCutFailureCode::SerializationFailure, message)
}

fn require_nonzero(label: &str, digest: SemanticCutDigest) -> Result<(), SemanticCutError> {
    if digest == [0; 32] {
        Err(cut_error(
            SemanticCutFailureCode::MissingBinding,
            format!("{label} digest is zero"),
        ))
    } else {
        Ok(())
    }
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, SemanticCutError> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > SEMANTIC_CUT_MAX_CANONICAL_BYTES {
        return Err(cut_error(
            SemanticCutFailureCode::CanonicalPayloadTooLarge,
            "semantic-cut canonical payload exceeds the frozen byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, SemanticCutError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > SEMANTIC_CUT_MAX_CANONICAL_BYTES {
        return Err(cut_error(
            SemanticCutFailureCode::CanonicalPayloadTooLarge,
            "semantic-cut canonical payload exceeds the frozen byte bound",
        ));
    }
    let value: T = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(cut_error(
            SemanticCutFailureCode::NonCanonicalEncoding,
            "semantic-cut payload is not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_serialized(
    domain: &[u8],
    value: &impl Serialize,
) -> Result<SemanticCutDigest, SemanticCutError> {
    Ok(digest_parts(domain, &[&canonical_bytes(value)?]))
}

fn digest_value(domain: &[u8], value: &Value) -> SemanticCutDigest {
    digest_parts(domain, &[canonical_json(value).as_bytes()])
}

fn digest_parts(domain: &[u8], parts: &[&[u8]]) -> SemanticCutDigest {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(domain);
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    sha256(&bytes)
}

fn certificate_digest(
    contract_version: u16,
    claim_digest: SemanticCutDigest,
    evidence_digest: SemanticCutDigest,
    verifier_identity_digest: SemanticCutDigest,
    terminal_rcq_identity_digest: SemanticCutDigest,
) -> SemanticCutDigest {
    digest_parts(
        CERTIFICATE_DOMAIN,
        &[
            &contract_version.to_be_bytes(),
            &claim_digest,
            &evidence_digest,
            &verifier_identity_digest,
            &terminal_rcq_identity_digest,
        ],
    )
}
