//! Proof-carrying exact RCQ and reasoning-epoch contraction contract.
//!
//! The trusted path supports only exact continuation identity. A clean restart,
//! approximate continuation, or classifier score cannot mint this capability;
//! those paths require a quality guard and frozen-baseline deoptimization.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zero_abi::{canonical_json, sha256};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};

pub type SemanticCutDigestV1 = [u8; 32];

pub const SEMANTIC_CUT_CONTRACT_VERSION_V1: u16 = 1;
pub const SEMANTIC_CUT_SCHEMA_VERSION_V1: &str = "racc-r-semantic-cut/v1";
pub const SEMANTIC_CUT_MAX_CANONICAL_BYTES_V1: usize = 64 * 1024;

const CLAIM_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.claim.v1\0";
const CERTIFICATE_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.certificate.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.contract.v1\0";
const RCQ_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.rcq_identity.v1\0";
const PROJECT_RELATION_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.project_relation.v1\0";
const EFFECT_RELATION_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.effect_relation.v1\0";
const VERIFIER_DOMAIN_V1: &[u8] = b"zerostack.semantic_cut.verifier_identity.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStateStatusV1 {
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
pub enum SemanticAuthorityV1 {
    DeterministicOnly,
    TaskSemanticSelection,
}

/// Typed epoch boundary. This is a state record, not proof by itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningSafepointV1 {
    schema_version: String,
    project_control_root: SemanticCutDigestV1,
    world_fiber_digest: SemanticCutDigestV1,
    evidence_state_digest: SemanticCutDigestV1,
    reasoning_contract_digest: SemanticCutDigestV1,
    fixed_model_digest: SemanticCutDigestV1,
    opaque_reasoning_state_digest: SemanticCutDigestV1,
    reasoning_state_status: ReasoningStateStatusV1,
    open_obligations_digest: SemanticCutDigestV1,
    resource_risk_reserve_digest: SemanticCutDigestV1,
    baseline_replay_cursor_digest: SemanticCutDigestV1,
    transaction_root_digest: SemanticCutDigestV1,
    receipt_head_digest: SemanticCutDigestV1,
}

impl ReasoningSafepointV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_control_root: SemanticCutDigestV1,
        world_fiber_digest: SemanticCutDigestV1,
        evidence_state_digest: SemanticCutDigestV1,
        reasoning_contract_digest: SemanticCutDigestV1,
        fixed_model_digest: SemanticCutDigestV1,
        opaque_reasoning_state_digest: SemanticCutDigestV1,
        reasoning_state_status: ReasoningStateStatusV1,
        open_obligations_digest: SemanticCutDigestV1,
        resource_risk_reserve_digest: SemanticCutDigestV1,
        baseline_replay_cursor_digest: SemanticCutDigestV1,
        transaction_root_digest: SemanticCutDigestV1,
        receipt_head_digest: SemanticCutDigestV1,
    ) -> Result<Self, SemanticCutErrorV1> {
        let value = Self {
            schema_version: SEMANTIC_CUT_SCHEMA_VERSION_V1.into(),
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

    pub fn validate(&self) -> Result<(), SemanticCutErrorV1> {
        if self.schema_version != SEMANTIC_CUT_SCHEMA_VERSION_V1 {
            return Err(cut_error(
                SemanticCutFailureCodeV1::SchemaVersionMismatch,
                "reasoning safepoint schema version is not v1",
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SemanticCutErrorV1> {
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SemanticCutErrorV1> {
        let safepoint: Self = decode_canonical(bytes)?;
        safepoint.validate()?;
        Ok(safepoint)
    }

    pub fn digest(&self) -> Result<SemanticCutDigestV1, SemanticCutErrorV1> {
        digest_serialized(b"zerostack.semantic_cut.safepoint.v1\0", self)
    }

    pub const fn project_control_root(&self) -> SemanticCutDigestV1 {
        self.project_control_root
    }
    pub const fn reasoning_contract_digest(&self) -> SemanticCutDigestV1 {
        self.reasoning_contract_digest
    }
    pub const fn fixed_model_digest(&self) -> SemanticCutDigestV1 {
        self.fixed_model_digest
    }
    pub const fn opaque_reasoning_state_digest(&self) -> SemanticCutDigestV1 {
        self.opaque_reasoning_state_digest
    }
    pub const fn reasoning_state_status(&self) -> ReasoningStateStatusV1 {
        self.reasoning_state_status
    }
    pub const fn receipt_head_digest(&self) -> SemanticCutDigestV1 {
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
pub struct SemanticCutClaimV1 {
    schema_version: String,
    input_project_control_root: SemanticCutDigestV1,
    baseline_epoch_class_digest: SemanticCutDigestV1,
    compiled_plan_digest: SemanticCutDigestV1,
    semantic_authority: SemanticAuthorityV1,
    baseline_terminal: ReasoningSafepointV1,
    compiled_terminal: ReasoningSafepointV1,
    baseline_external_effects_digest: SemanticCutDigestV1,
    compiled_external_effects_digest: SemanticCutDigestV1,
    baseline_attribution_identity_digest: SemanticCutDigestV1,
    compiled_attribution_identity_digest: SemanticCutDigestV1,
    baseline_resource_receipt_digest: SemanticCutDigestV1,
    compiled_resource_receipt_digest: SemanticCutDigestV1,
    comparison_identity_digest: SemanticCutDigestV1,
    certificate_scope_digest: SemanticCutDigestV1,
    deoptimization_map_digest: SemanticCutDigestV1,
}

impl SemanticCutClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new_exact(
        input_project_control_root: SemanticCutDigestV1,
        baseline_epoch_class_digest: SemanticCutDigestV1,
        compiled_plan_digest: SemanticCutDigestV1,
        baseline_terminal: ReasoningSafepointV1,
        compiled_terminal: ReasoningSafepointV1,
        baseline_external_effects_digest: SemanticCutDigestV1,
        compiled_external_effects_digest: SemanticCutDigestV1,
        baseline_attribution_identity_digest: SemanticCutDigestV1,
        compiled_attribution_identity_digest: SemanticCutDigestV1,
        baseline_resource_receipt_digest: SemanticCutDigestV1,
        compiled_resource_receipt_digest: SemanticCutDigestV1,
        comparison_identity_digest: SemanticCutDigestV1,
        certificate_scope_digest: SemanticCutDigestV1,
        deoptimization_map_digest: SemanticCutDigestV1,
    ) -> Result<Self, SemanticCutErrorV1> {
        let claim = Self {
            schema_version: SEMANTIC_CUT_SCHEMA_VERSION_V1.into(),
            input_project_control_root,
            baseline_epoch_class_digest,
            compiled_plan_digest,
            semantic_authority: SemanticAuthorityV1::DeterministicOnly,
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

    pub fn validate_exact(&self) -> Result<(), SemanticCutErrorV1> {
        if self.schema_version != SEMANTIC_CUT_SCHEMA_VERSION_V1 {
            return Err(cut_error(
                SemanticCutFailureCodeV1::SchemaVersionMismatch,
                "semantic-cut claim schema version is not v1",
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
        if self.semantic_authority != SemanticAuthorityV1::DeterministicOnly {
            return Err(cut_error(
                SemanticCutFailureCodeV1::SemanticAuthorityCrossing,
                "semantic-cut plan may contain only declared deterministic operations",
            ));
        }
        if self.baseline_terminal.reasoning_state_status != ReasoningStateStatusV1::ExactPreserved
            || self.compiled_terminal.reasoning_state_status
                != ReasoningStateStatusV1::ExactPreserved
        {
            return Err(cut_error(
                SemanticCutFailureCodeV1::ContinuationNotExact,
                "strict epoch contraction requires exact preserved reasoning state",
            ));
        }
        if !self
            .baseline_terminal
            .exact_continuation_equal(&self.compiled_terminal)
        {
            return Err(cut_error(
                SemanticCutFailureCodeV1::TerminalStateMismatch,
                "baseline and compiled terminal protected state are not identical",
            ));
        }
        if self.baseline_external_effects_digest != self.compiled_external_effects_digest {
            return Err(cut_error(
                SemanticCutFailureCodeV1::ExternalEffectMismatch,
                "baseline and compiled external-effect identities differ",
            ));
        }
        if self.baseline_attribution_identity_digest != self.compiled_attribution_identity_digest {
            return Err(cut_error(
                SemanticCutFailureCodeV1::AttributionMismatch,
                "compiled transition changes semantic attribution",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SemanticCutErrorV1> {
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SemanticCutErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate_exact()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<SemanticCutDigestV1, SemanticCutErrorV1> {
        digest_serialized(CLAIM_DOMAIN_V1, self)
    }

    pub fn terminal_rcq_identity_digest(&self) -> SemanticCutDigestV1 {
        let terminal = &self.compiled_terminal;
        digest_parts(
            RCQ_DOMAIN_V1,
            &[
                &terminal.fixed_model_digest,
                &terminal.reasoning_contract_digest,
                &terminal.opaque_reasoning_state_digest,
            ],
        )
    }

    pub fn terminal_project_relation_digest(&self) -> SemanticCutDigestV1 {
        digest_parts(
            PROJECT_RELATION_DOMAIN_V1,
            &[
                &self.baseline_terminal.project_control_root,
                &self.compiled_terminal.project_control_root,
            ],
        )
    }

    pub fn external_effect_relation_digest(&self) -> SemanticCutDigestV1 {
        digest_parts(
            EFFECT_RELATION_DOMAIN_V1,
            &[
                &self.baseline_external_effects_digest,
                &self.compiled_external_effects_digest,
            ],
        )
    }

    pub const fn input_project_control_root(&self) -> SemanticCutDigestV1 {
        self.input_project_control_root
    }
    pub const fn compiled_plan_digest(&self) -> SemanticCutDigestV1 {
        self.compiled_plan_digest
    }
    pub const fn comparison_identity_digest(&self) -> SemanticCutDigestV1 {
        self.comparison_identity_digest
    }
    pub const fn certificate_scope_digest(&self) -> SemanticCutDigestV1 {
        self.certificate_scope_digest
    }
    pub const fn reasoning_contract_digest(&self) -> SemanticCutDigestV1 {
        self.compiled_terminal.reasoning_contract_digest
    }
    pub const fn fixed_model_digest(&self) -> SemanticCutDigestV1 {
        self.compiled_terminal.fixed_model_digest
    }
    pub const fn attribution_identity_digest(&self) -> SemanticCutDigestV1 {
        self.compiled_attribution_identity_digest
    }
    pub const fn deoptimization_map_digest(&self) -> SemanticCutDigestV1 {
        self.deoptimization_map_digest
    }
}

/// Opaque G3 authority. Only exact claim validation plus verified bytes can mint it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCutEvidenceV1 {
    contract_version: u16,
    claim: SemanticCutClaimV1,
    claim_digest: SemanticCutDigestV1,
    evidence_digest: SemanticCutDigestV1,
    verifier_identity_digest: SemanticCutDigestV1,
    certificate_digest: SemanticCutDigestV1,
}

impl SemanticCutEvidenceV1 {
    pub fn verify_owner_scoped(
        claim: SemanticCutClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, SemanticCutErrorV1> {
        claim.validate_exact()?;
        match (evidence.query(), &evidence.certificate().completeness) {
            (
                Query::BuildReceipt { .. },
                CompletenessWitness::BuildReceipt { exit_code: 0, .. },
            )
            | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
            _ => {
                return Err(cut_error(
                    SemanticCutFailureCodeV1::UnsupportedEvidenceClass,
                    "semantic-cut authority requires a successful verified build or test trace",
                ));
            }
        }
        let canonical_claim = claim.canonical_bytes()?;
        if evidence.payload() != canonical_claim {
            return Err(cut_error(
                SemanticCutFailureCodeV1::EvidencePayloadMismatch,
                "verified evidence payload is not the exact canonical semantic-cut claim",
            ));
        }
        let claim_digest = claim.digest()?;
        let evidence_digest = evidence
            .certificate()
            .canonical_digest()
            .map_err(|error| json_error(error.to_string()))?;
        let verifier_identity_digest = semantic_cut_verifier_identity_v1(evidence);
        let certificate_digest = certificate_digest(
            SEMANTIC_CUT_CONTRACT_VERSION_V1,
            claim_digest,
            evidence_digest,
            verifier_identity_digest,
            claim.terminal_rcq_identity_digest(),
        );
        Ok(Self {
            contract_version: SEMANTIC_CUT_CONTRACT_VERSION_V1,
            claim,
            claim_digest,
            evidence_digest,
            verifier_identity_digest,
            certificate_digest,
        })
    }

    pub fn validate(&self) -> Result<(), SemanticCutErrorV1> {
        if self.contract_version != SEMANTIC_CUT_CONTRACT_VERSION_V1 {
            return Err(cut_error(
                SemanticCutFailureCodeV1::SchemaVersionMismatch,
                "semantic-cut certificate contract version is not v1",
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
                SemanticCutFailureCodeV1::CertificateDigestMismatch,
                "semantic-cut certificate digest does not bind its claim and evidence",
            ));
        }
        Ok(())
    }

    pub fn record(&self) -> SemanticCutCertificateRecordV1 {
        SemanticCutCertificateRecordV1 {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
    }

    pub const fn claim(&self) -> &SemanticCutClaimV1 {
        &self.claim
    }
    pub const fn certificate_digest(&self) -> SemanticCutDigestV1 {
        self.certificate_digest
    }
    pub const fn verifier_identity_digest(&self) -> SemanticCutDigestV1 {
        self.verifier_identity_digest
    }
    pub fn terminal_rcq_identity_digest(&self) -> SemanticCutDigestV1 {
        self.claim.terminal_rcq_identity_digest()
    }
}

/// Public receipt form. It is replay-validatable but cannot authorize execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCutCertificateRecordV1 {
    pub contract_version: u16,
    pub claim: SemanticCutClaimV1,
    pub claim_digest: SemanticCutDigestV1,
    pub evidence_digest: SemanticCutDigestV1,
    pub verifier_identity_digest: SemanticCutDigestV1,
    pub certificate_digest: SemanticCutDigestV1,
}

impl SemanticCutCertificateRecordV1 {
    pub fn validate(&self) -> Result<(), SemanticCutErrorV1> {
        let certificate = SemanticCutEvidenceV1 {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        };
        certificate.validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SemanticCutErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SemanticCutErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

pub fn semantic_cut_verifier_identity_v1(
    evidence: &VerifiedEvidence<'_, '_>,
) -> SemanticCutDigestV1 {
    let provenance = evidence.provenance();
    let body = json!({
        "index_id": provenance.index_id,
        "index_version": provenance.index_version,
        "operator_id": provenance.operator_id,
        "operator_version": provenance.operator_version,
        "parser_id": provenance.parser_id,
        "parser_version": provenance.parser_version,
    });
    digest_value(VERIFIER_DOMAIN_V1, &body)
}

pub fn semantic_cut_contract_manifest_v1() -> Value {
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
        "contract_version": SEMANTIC_CUT_CONTRACT_VERSION_V1,
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
        "name": "zerostack.semantic_cut.v1",
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
        "schema_version": SEMANTIC_CUT_SCHEMA_VERSION_V1,
        "strict_proof_mode": "exact_continuation_identity_only",
    })
}

pub fn semantic_cut_contract_digest_v1() -> SemanticCutDigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &semantic_cut_contract_manifest_v1())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticCutFailureCodeV1 {
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
pub struct SemanticCutErrorV1 {
    failure_code: SemanticCutFailureCodeV1,
    message: String,
}

impl SemanticCutErrorV1 {
    pub const fn failure_code(&self) -> SemanticCutFailureCodeV1 {
        self.failure_code
    }
}

impl fmt::Display for SemanticCutErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.failure_code, self.message)
    }
}
impl Error for SemanticCutErrorV1 {}

fn cut_error(
    failure_code: SemanticCutFailureCodeV1,
    message: impl Into<String>,
) -> SemanticCutErrorV1 {
    SemanticCutErrorV1 {
        failure_code,
        message: message.into(),
    }
}

fn json_error(message: impl Into<String>) -> SemanticCutErrorV1 {
    cut_error(SemanticCutFailureCodeV1::SerializationFailure, message)
}

fn require_nonzero(label: &str, digest: SemanticCutDigestV1) -> Result<(), SemanticCutErrorV1> {
    if digest == [0; 32] {
        Err(cut_error(
            SemanticCutFailureCodeV1::MissingBinding,
            format!("{label} digest is zero"),
        ))
    } else {
        Ok(())
    }
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, SemanticCutErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > SEMANTIC_CUT_MAX_CANONICAL_BYTES_V1 {
        return Err(cut_error(
            SemanticCutFailureCodeV1::CanonicalPayloadTooLarge,
            "semantic-cut canonical payload exceeds the frozen byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, SemanticCutErrorV1>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > SEMANTIC_CUT_MAX_CANONICAL_BYTES_V1 {
        return Err(cut_error(
            SemanticCutFailureCodeV1::CanonicalPayloadTooLarge,
            "semantic-cut canonical payload exceeds the frozen byte bound",
        ));
    }
    let value: T = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(cut_error(
            SemanticCutFailureCodeV1::NonCanonicalEncoding,
            "semantic-cut payload is not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_serialized(
    domain: &[u8],
    value: &impl Serialize,
) -> Result<SemanticCutDigestV1, SemanticCutErrorV1> {
    Ok(digest_parts(domain, &[&canonical_bytes(value)?]))
}

fn digest_value(domain: &[u8], value: &Value) -> SemanticCutDigestV1 {
    digest_parts(domain, &[canonical_json(value).as_bytes()])
}

fn digest_parts(domain: &[u8], parts: &[&[u8]]) -> SemanticCutDigestV1 {
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
    claim_digest: SemanticCutDigestV1,
    evidence_digest: SemanticCutDigestV1,
    verifier_identity_digest: SemanticCutDigestV1,
    terminal_rcq_identity_digest: SemanticCutDigestV1,
) -> SemanticCutDigestV1 {
    digest_parts(
        CERTIFICATE_DOMAIN_V1,
        &[
            &contract_version.to_be_bytes(),
            &claim_digest,
            &evidence_digest,
            &verifier_identity_digest,
            &terminal_rcq_identity_digest,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> SemanticCutDigestV1 {
        [byte; 32]
    }

    fn safepoint(receipt: u8) -> ReasoningSafepointV1 {
        ReasoningSafepointV1::new(
            digest(1),
            digest(2),
            digest(3),
            digest(4),
            digest(5),
            digest(6),
            ReasoningStateStatusV1::ExactPreserved,
            digest(7),
            digest(8),
            digest(9),
            digest(10),
            digest(receipt),
        )
        .unwrap()
    }

    fn exact_claim() -> SemanticCutClaimV1 {
        SemanticCutClaimV1::new_exact(
            digest(11),
            digest(12),
            digest(13),
            safepoint(14),
            safepoint(15),
            digest(16),
            digest(16),
            digest(17),
            digest(17),
            digest(18),
            digest(19),
            digest(20),
            digest(21),
            digest(22),
        )
        .unwrap()
    }

    #[test]
    fn exact_epoch_claim_is_canonical_and_receipt_heads_may_differ() {
        let claim = exact_claim();
        claim.validate_exact().unwrap();
        assert_ne!(
            claim.baseline_terminal.receipt_head_digest(),
            claim.compiled_terminal.receipt_head_digest()
        );
        let bytes = claim.canonical_bytes().unwrap();
        assert_eq!(
            SemanticCutClaimV1::from_canonical_bytes(&bytes).unwrap(),
            claim
        );
        let mut noncanonical = bytes;
        noncanonical.push(b'\n');
        assert_eq!(
            SemanticCutClaimV1::from_canonical_bytes(&noncanonical)
                .unwrap_err()
                .failure_code(),
            SemanticCutFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn clean_restart_and_approximate_state_never_mint_exact_claims() {
        for status in [
            ReasoningStateStatusV1::ExactCleanRestart,
            ReasoningStateStatusV1::ScopedEquivalent,
            ReasoningStateStatusV1::Approximate,
            ReasoningStateStatusV1::Unavailable,
            ReasoningStateStatusV1::Expired,
            ReasoningStateStatusV1::IdentityMismatch,
        ] {
            let mut claim = exact_claim();
            claim.compiled_terminal.reasoning_state_status = status;
            assert_eq!(
                claim.validate_exact().unwrap_err().failure_code(),
                SemanticCutFailureCodeV1::ContinuationNotExact
            );
        }
    }

    #[test]
    fn every_protected_terminal_relation_fails_closed() {
        let mut claim = exact_claim();
        claim.compiled_terminal.opaque_reasoning_state_digest = digest(99);
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::TerminalStateMismatch
        );
        let mut claim = exact_claim();
        claim.compiled_external_effects_digest = digest(99);
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::ExternalEffectMismatch
        );
        let mut claim = exact_claim();
        claim.compiled_attribution_identity_digest = digest(99);
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::AttributionMismatch
        );
        let mut claim = exact_claim();
        claim.semantic_authority = SemanticAuthorityV1::TaskSemanticSelection;
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::SemanticAuthorityCrossing
        );
    }

    #[test]
    fn contract_and_claim_digests_are_stable() {
        assert_eq!(
            hex(&semantic_cut_contract_digest_v1()),
            "5701b3a000c045c39d86886801c7abbc9f4cf651b20ba47ba8ac3964fce88c6a"
        );
        assert_eq!(
            hex(&exact_claim().digest().unwrap()),
            "249d8029a25780f819b1c70dbbfd04faaaef7adbc0997a365c7c967599a83894"
        );
    }

    fn hex(bytes: &SemanticCutDigestV1) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
