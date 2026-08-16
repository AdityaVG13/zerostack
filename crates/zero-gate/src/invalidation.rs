//! Proof-carrying Robust Snap and causal-artifact invalidation intake.
//!
//! Engine-authored descriptors remain replay records. Only successful exact
//! verifier evidence can mint opaque ZeroStack authority for a complete world
//! fiber or protected support closure. Value equality, support validity, and
//! reuse economics remain distinct decisions.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{
    ArtifactOwnerV1, CertifiedInfluenceClosure, DigestV1, RobustSnapCertificate, SnapLevel,
    canonical_json, freshness_contract_digest_v1, robust_snap_contract_digest_v1, sha256,
};
use zero_cert::VerifiedEvidence;

use crate::q99::{
    CausalCacheBindingV1, q99_contract_digest_v2, q99_verifier_identity_v1,
    verified_evidence_digest, verify_exact_successful_payload,
};

pub const INVALIDATION_INTAKE_CONTRACT_VERSION_V1: u16 = 1;
pub const ROBUST_SNAP_INTAKE_SCHEMA_VERSION_V1: &str = "zerostack.robust_snap.intake_claim.v1";
pub const CAUSAL_ARTIFACT_SCHEMA_VERSION_V1: &str = "zerostack.causal_artifact.intake_claim.v1";
pub const ROBUST_SNAP_INTAKE_SCHEMA_SHA256_V1: &str =
    "3a9a8056807e143daff4dd3713d73226ecb0ff36981e6307ea0eab744d4ff180";
pub const CAUSAL_ARTIFACT_SCHEMA_SHA256_V1: &str =
    "5a7939a88a545b0cf8a5abdd26412a24aa5b8ab381f2172fbfed7bc73a0d893f";
pub const INVALIDATION_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const INVALIDATION_MAX_SUPPORT_ROOTS_V1: usize = 4_096;

const SNAP_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.robust_snap.intake_claim.v1\0";
const SNAP_RECORD_DOMAIN_V1: &[u8] = b"zerostack.robust_snap.intake_record.v1\0";
const ARTIFACT_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.causal_artifact.intake_claim.v1\0";
const ARTIFACT_RECORD_DOMAIN_V1: &[u8] = b"zerostack.causal_artifact.intake_record.v1\0";
const CACHE_BINDING_DOMAIN_V1: &[u8] = b"zerostack.causal_artifact.cache_binding.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.invalidation_intake.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapFiberRepresentationV1 {
    FiniteExact,
    ConservativeSuperset,
    Unknown,
}

impl SnapFiberRepresentationV1 {
    const fn is_complete(self) -> bool {
        matches!(self, Self::FiniteExact | Self::ConservativeSuperset)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RobustSnapIntakeClaimV1 {
    schema_version: String,
    snap_certificate_digest: DigestV1,
    fiber_representation: SnapFiberRepresentationV1,
    fiber_completeness_receipt_digest: DigestV1,
    protected_use_scope_digest: DigestV1,
    dominance_scope_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl RobustSnapIntakeClaimV1 {
    pub fn new(
        snap_certificate_digest: DigestV1,
        fiber_representation: SnapFiberRepresentationV1,
        fiber_completeness_receipt_digest: DigestV1,
        protected_use_scope_digest: DigestV1,
        dominance_scope_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, InvalidationIntakeErrorV1> {
        let claim = Self {
            schema_version: ROBUST_SNAP_INTAKE_SCHEMA_VERSION_V1.into(),
            snap_certificate_digest,
            fiber_representation,
            fiber_completeness_receipt_digest,
            protected_use_scope_digest,
            dominance_scope_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), InvalidationIntakeErrorV1> {
        if self.schema_version != ROBUST_SNAP_INTAKE_SCHEMA_VERSION_V1 {
            return Err(intake_error(
                InvalidationFailureCodeV1::SchemaVersionMismatch,
                "Robust Snap intake schema version is not v1",
            ));
        }
        require_nonzero(
            "Robust Snap intake",
            &[
                self.snap_certificate_digest,
                self.fiber_completeness_receipt_digest,
                self.protected_use_scope_digest,
                self.dominance_scope_digest,
                self.verifier_identity_digest,
            ],
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, InvalidationIntakeErrorV1> {
        Ok(domain_digest(
            SNAP_CLAIM_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RobustSnapIntakeDispositionV1 {
    Complete,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RobustSnapIntakeRecordV1 {
    pub contract_version: u16,
    pub claim: RobustSnapIntakeClaimV1,
    pub claim_digest: DigestV1,
    pub snap_certificate: RobustSnapCertificate,
    pub snap_certificate_bytes_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub disposition: RobustSnapIntakeDispositionV1,
    pub authority_digest: DigestV1,
}

impl RobustSnapIntakeRecordV1 {
    pub fn validate(&self) -> Result<(), InvalidationIntakeErrorV1> {
        if self.contract_version != INVALIDATION_INTAKE_CONTRACT_VERSION_V1 {
            return Err(intake_error(
                InvalidationFailureCodeV1::SchemaVersionMismatch,
                "Robust Snap intake contract version is not v1",
            ));
        }
        self.claim.validate()?;
        validate_snap_certificate(&self.snap_certificate)?;
        let snap_bytes = self.snap_certificate.canonical_bytes().map_err(|error| {
            intake_error(
                InvalidationFailureCodeV1::InvalidRobustSnap,
                error.to_string(),
            )
        })?;
        let expected_disposition = if self.claim.fiber_representation.is_complete() {
            RobustSnapIntakeDispositionV1::Complete
        } else {
            RobustSnapIntakeDispositionV1::Unknown
        };
        require_nonzero(
            "Robust Snap intake record",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.claim.digest()? != self.claim_digest
            || self.claim.snap_certificate_digest != self.snap_certificate.certificate_digest
            || domain_digest(b"zerostack.robust_snap.canonical_bytes.v1\0", &snap_bytes)
                != self.snap_certificate_bytes_digest
            || self.disposition != expected_disposition
            || self.expected_authority_digest()? != self.authority_digest
        {
            return Err(intake_error(
                InvalidationFailureCodeV1::RecordDigestMismatch,
                "Robust Snap intake record does not replay",
            ));
        }
        Ok(())
    }

    fn expected_authority_digest(&self) -> Result<DigestV1, InvalidationIntakeErrorV1> {
        digest_serialized(
            SNAP_RECORD_DOMAIN_V1,
            &json!({
                "claim_digest": self.claim_digest,
                "contract_version": self.contract_version,
                "disposition": self.disposition,
                "evidence_digest": self.evidence_digest,
                "snap_certificate_bytes_digest": self.snap_certificate_bytes_digest,
            }),
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, InvalidationIntakeErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
pub struct VerifiedRobustSnapAuthorityV1 {
    record: RobustSnapIntakeRecordV1,
}

impl VerifiedRobustSnapAuthorityV1 {
    pub const fn record(&self) -> &RobustSnapIntakeRecordV1 {
        &self.record
    }

    /// Robust Snap only proves protected decision geometry. It never executes an effect.
    pub const fn permits_operational_execution(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub enum RobustSnapIntakeDecisionV1 {
    Complete(VerifiedRobustSnapAuthorityV1),
    Unknown(RobustSnapIntakeRecordV1),
}

pub fn verify_robust_snap_intake_v1(
    claim: RobustSnapIntakeClaimV1,
    snap_certificate: RobustSnapCertificate,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<RobustSnapIntakeDecisionV1, InvalidationIntakeErrorV1> {
    claim.validate()?;
    validate_snap_certificate(&snap_certificate)?;
    if claim.snap_certificate_digest != snap_certificate.certificate_digest {
        return Err(intake_error(
            InvalidationFailureCodeV1::BindingMismatch,
            "Robust Snap claim does not bind the supplied certificate",
        ));
    }
    if q99_verifier_identity_v1(evidence) != claim.verifier_identity_digest {
        return Err(intake_error(
            InvalidationFailureCodeV1::VerifierIdentityMismatch,
            "Robust Snap verifier differs from its evidence route",
        ));
    }
    let envelope = snap_envelope_bytes(&claim, &snap_certificate)?;
    verify_exact_successful_payload(&envelope, evidence).map_err(map_q99_evidence_error)?;
    let snap_bytes = snap_certificate.canonical_bytes().map_err(|error| {
        intake_error(
            InvalidationFailureCodeV1::InvalidRobustSnap,
            error.to_string(),
        )
    })?;
    let disposition = if claim.fiber_representation.is_complete() {
        RobustSnapIntakeDispositionV1::Complete
    } else {
        RobustSnapIntakeDispositionV1::Unknown
    };
    let mut record = RobustSnapIntakeRecordV1 {
        contract_version: INVALIDATION_INTAKE_CONTRACT_VERSION_V1,
        claim_digest: claim.digest()?,
        claim,
        snap_certificate,
        snap_certificate_bytes_digest: domain_digest(
            b"zerostack.robust_snap.canonical_bytes.v1\0",
            &snap_bytes,
        ),
        evidence_digest: verified_evidence_digest(evidence).map_err(map_q99_evidence_error)?,
        disposition,
        authority_digest: DigestV1::ZERO,
    };
    record.authority_digest = record.expected_authority_digest()?;
    record.validate()?;
    Ok(match disposition {
        RobustSnapIntakeDispositionV1::Complete => {
            RobustSnapIntakeDecisionV1::Complete(VerifiedRobustSnapAuthorityV1 { record })
        }
        RobustSnapIntakeDispositionV1::Unknown => RobustSnapIntakeDecisionV1::Unknown(record),
    })
}

fn validate_snap_certificate(
    certificate: &RobustSnapCertificate,
) -> Result<(), InvalidationIntakeErrorV1> {
    certificate.validate().map_err(|error| {
        intake_error(
            InvalidationFailureCodeV1::InvalidRobustSnap,
            error.to_string(),
        )
    })?;
    require_nonzero(
        "Robust Snap frozen identity",
        &[
            certificate.fiber.assembly_manifest_digest,
            certificate.fiber.source_image_digest,
            certificate.fiber.task_fingerprint,
            certificate.certificate_digest,
        ],
    )?;
    for effect in certificate
        .protected_effects
        .iter()
        .flat_map(|set| set.effects.iter())
        .chain(certificate.first_turn_selectable.iter())
        .chain(certificate.expressible_and_verifiable.iter())
    {
        require_nonzero("Robust Snap effect", &[effect.effect_digest])?;
    }
    if let Some(selected) = &certificate.selected_effect {
        require_nonzero("Robust Snap selected effect", &[selected.effect_digest])?;
    }
    if let Some(tree) = &certificate.evidence_tree {
        require_nonzero("Robust Snap evidence tree", &[tree.evidence_schema_digest])?;
        for observation in tree.leaves.iter().flat_map(|leaf| leaf.path.iter()) {
            require_nonzero(
                "Robust Snap evidence observation",
                &[observation.evidence_id, observation.outcome_digest],
            )?;
        }
    }
    if !matches!(certificate.snap_level, SnapLevel::S0 | SnapLevel::S1) {
        return Err(intake_error(
            InvalidationFailureCodeV1::InvalidRobustSnap,
            "Unknown Robust Snap level cannot enter proof-carrying intake",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportCompletenessClassV1 {
    Exact,
    SoundOverapproximation,
    Heuristic,
    Unknown,
}

impl SupportCompletenessClassV1 {
    const fn authorizes_protected_support(self) -> bool {
        matches!(self, Self::Exact | Self::SoundOverapproximation)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DerivationAuthorityV1 {
    Witness { witness_digest: DigestV1 },
    ReplayRecipe { recipe_digest: DigestV1 },
    OpaqueWholeUnit { unit_root_digest: DigestV1 },
}

impl DerivationAuthorityV1 {
    fn digest(&self) -> DigestV1 {
        match self {
            Self::Witness { witness_digest } => *witness_digest,
            Self::ReplayRecipe { recipe_digest } => *recipe_digest,
            Self::OpaqueWholeUnit { unit_root_digest } => *unit_root_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalArtifactIntakeClaimV1 {
    schema_version: String,
    artifact_digest: DigestV1,
    artifact_owner: ArtifactOwnerV1,
    producer_identity_digest: DigestV1,
    declared_support_roots: Vec<DigestV1>,
    support_closure_digest: DigestV1,
    support_class: SupportCompletenessClassV1,
    derivation_authority: DerivationAuthorityV1,
    invalidation_predicate_digest: DigestV1,
    protected_use_scope_digest: DigestV1,
    verifier_scope_digest: DigestV1,
    validation_cost_profile_digest: DigestV1,
    recovery_route_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl CausalArtifactIntakeClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_digest: DigestV1,
        artifact_owner: ArtifactOwnerV1,
        producer_identity_digest: DigestV1,
        declared_support_roots: Vec<DigestV1>,
        support_closure_digest: DigestV1,
        support_class: SupportCompletenessClassV1,
        derivation_authority: DerivationAuthorityV1,
        invalidation_predicate_digest: DigestV1,
        protected_use_scope_digest: DigestV1,
        verifier_scope_digest: DigestV1,
        validation_cost_profile_digest: DigestV1,
        recovery_route_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, InvalidationIntakeErrorV1> {
        let claim = Self {
            schema_version: CAUSAL_ARTIFACT_SCHEMA_VERSION_V1.into(),
            artifact_digest,
            artifact_owner,
            producer_identity_digest,
            declared_support_roots,
            support_closure_digest,
            support_class,
            derivation_authority,
            invalidation_predicate_digest,
            protected_use_scope_digest,
            verifier_scope_digest,
            validation_cost_profile_digest,
            recovery_route_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), InvalidationIntakeErrorV1> {
        if self.schema_version != CAUSAL_ARTIFACT_SCHEMA_VERSION_V1 {
            return Err(intake_error(
                InvalidationFailureCodeV1::SchemaVersionMismatch,
                "causal artifact intake schema version is not v1",
            ));
        }
        if self.declared_support_roots.is_empty()
            || self.declared_support_roots.len() > INVALIDATION_MAX_SUPPORT_ROOTS_V1
            || self
                .declared_support_roots
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(intake_error(
                InvalidationFailureCodeV1::InvalidSupportRoots,
                "declared support roots must be nonempty, bounded, sorted, and unique",
            ));
        }
        require_nonzero(
            "causal artifact intake",
            &[
                self.artifact_digest,
                self.producer_identity_digest,
                self.support_closure_digest,
                self.derivation_authority.digest(),
                self.invalidation_predicate_digest,
                self.protected_use_scope_digest,
                self.verifier_scope_digest,
                self.validation_cost_profile_digest,
                self.recovery_route_digest,
                self.verifier_identity_digest,
            ],
        )?;
        require_nonzero("declared support root", &self.declared_support_roots)?;
        if self.support_class == SupportCompletenessClassV1::Exact
            && matches!(
                self.derivation_authority,
                DerivationAuthorityV1::OpaqueWholeUnit { .. }
            )
        {
            return Err(intake_error(
                InvalidationFailureCodeV1::SupportClassMismatch,
                "opaque whole-unit isolation is conservative, never exact support",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, InvalidationIntakeErrorV1> {
        Ok(domain_digest(
            ARTIFACT_CLAIM_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationIntakeDispositionV1 {
    ProtectedSupport,
    RetrievalOnly,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalArtifactIntakeRecordV1 {
    pub contract_version: u16,
    pub claim: CausalArtifactIntakeClaimV1,
    pub claim_digest: DigestV1,
    pub support_closure: CertifiedInfluenceClosure,
    pub support_closure_bytes_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub disposition: InvalidationIntakeDispositionV1,
    pub authority_digest: DigestV1,
}

impl CausalArtifactIntakeRecordV1 {
    pub fn validate(&self) -> Result<(), InvalidationIntakeErrorV1> {
        if self.contract_version != INVALIDATION_INTAKE_CONTRACT_VERSION_V1 {
            return Err(intake_error(
                InvalidationFailureCodeV1::SchemaVersionMismatch,
                "causal artifact intake contract version is not v1",
            ));
        }
        self.claim.validate()?;
        self.support_closure.validate().map_err(|error| {
            intake_error(
                InvalidationFailureCodeV1::InvalidSupportClosure,
                error.to_string(),
            )
        })?;
        let closure_bytes = self.support_closure.canonical_bytes().map_err(|error| {
            intake_error(
                InvalidationFailureCodeV1::InvalidSupportClosure,
                error.to_string(),
            )
        })?;
        let expected_disposition = disposition_for_support(self.claim.support_class);
        require_nonzero(
            "causal artifact intake record",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.claim.digest()? != self.claim_digest
            || self.claim.support_closure_digest != self.support_closure.certificate_digest
            || domain_digest(
                b"zerostack.causal_artifact.closure_bytes.v1\0",
                &closure_bytes,
            ) != self.support_closure_bytes_digest
            || self.disposition != expected_disposition
            || self.expected_authority_digest()? != self.authority_digest
        {
            return Err(intake_error(
                InvalidationFailureCodeV1::RecordDigestMismatch,
                "causal artifact intake record does not replay",
            ));
        }
        Ok(())
    }

    fn expected_authority_digest(&self) -> Result<DigestV1, InvalidationIntakeErrorV1> {
        digest_serialized(
            ARTIFACT_RECORD_DOMAIN_V1,
            &json!({
                "claim_digest": self.claim_digest,
                "contract_version": self.contract_version,
                "disposition": self.disposition,
                "evidence_digest": self.evidence_digest,
                "support_closure_bytes_digest": self.support_closure_bytes_digest,
            }),
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, InvalidationIntakeErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
pub struct ProofCarryingInvalidationAuthorityV1 {
    record: CausalArtifactIntakeRecordV1,
}

impl ProofCarryingInvalidationAuthorityV1 {
    pub const fn record(&self) -> &CausalArtifactIntakeRecordV1 {
        &self.record
    }

    pub fn bind_cache(
        &self,
        binding: &CausalCacheBindingV1,
    ) -> Result<BoundCausalCacheInvalidationV1, InvalidationIntakeErrorV1> {
        binding.validate().map_err(|error| {
            intake_error(
                InvalidationFailureCodeV1::BindingMismatch,
                error.to_string(),
            )
        })?;
        let claim = &self.record.claim;
        if claim.artifact_digest != binding.artifact_digest
            || claim.artifact_owner != binding.artifact_owner
            || claim.producer_identity_digest != binding.producer_contract_digest
            || claim.support_closure_digest != binding.dependency_root
            || claim.protected_use_scope_digest != binding.protected_use_class_digest
            || claim.verifier_scope_digest != binding.verifier_scope_digest
            || claim
                .declared_support_roots
                .binary_search(&binding.source_root)
                .is_err()
            || claim.recovery_route_digest != binding.recovery_route_digest
            || self.record.authority_digest != binding.invalidation_certificate_digest
        {
            return Err(intake_error(
                InvalidationFailureCodeV1::BindingMismatch,
                "causal artifact authority does not match the complete cache binding",
            ));
        }
        Ok(BoundCausalCacheInvalidationV1 {
            binding_digest: binding.digest().map_err(|error| {
                intake_error(
                    InvalidationFailureCodeV1::BindingMismatch,
                    error.to_string(),
                )
            })?,
            invalidation_authority_digest: self.record.authority_digest,
            support_class: claim.support_class,
            bound_digest: domain_digest(
                CACHE_BINDING_DOMAIN_V1,
                canonical_json(&json!({
                    "binding_digest": binding.digest().map_err(|error| intake_error(
                        InvalidationFailureCodeV1::BindingMismatch,
                        error.to_string(),
                    ))?,
                    "invalidation_authority_digest": self.record.authority_digest,
                    "support_class": claim.support_class,
                }))
                .as_bytes(),
            ),
        })
    }
}

#[derive(Debug)]
pub struct BoundCausalCacheInvalidationV1 {
    binding_digest: DigestV1,
    invalidation_authority_digest: DigestV1,
    support_class: SupportCompletenessClassV1,
    bound_digest: DigestV1,
}

impl BoundCausalCacheInvalidationV1 {
    pub(crate) fn authorizes(
        &self,
        binding: &CausalCacheBindingV1,
    ) -> Result<bool, InvalidationIntakeErrorV1> {
        let binding_digest = binding.digest().map_err(|error| {
            intake_error(
                InvalidationFailureCodeV1::BindingMismatch,
                error.to_string(),
            )
        })?;
        let expected_bound = domain_digest(
            CACHE_BINDING_DOMAIN_V1,
            canonical_json(&json!({
                "binding_digest": binding_digest,
                "invalidation_authority_digest": self.invalidation_authority_digest,
                "support_class": self.support_class,
            }))
            .as_bytes(),
        );
        Ok(self.support_class.authorizes_protected_support()
            && binding.invalidation_certificate_digest == self.invalidation_authority_digest
            && binding_digest == self.binding_digest
            && expected_bound == self.bound_digest)
    }

    #[cfg(test)]
    pub(crate) fn test_only(binding: &CausalCacheBindingV1) -> Self {
        let binding_digest = binding.digest().expect("test binding is valid");
        let support_class = SupportCompletenessClassV1::Exact;
        let invalidation_authority_digest = binding.invalidation_certificate_digest;
        let bound_digest = domain_digest(
            CACHE_BINDING_DOMAIN_V1,
            canonical_json(&json!({
                "binding_digest": binding_digest,
                "invalidation_authority_digest": invalidation_authority_digest,
                "support_class": support_class,
            }))
            .as_bytes(),
        );
        Self {
            binding_digest,
            invalidation_authority_digest,
            support_class,
            bound_digest,
        }
    }
}

#[derive(Debug)]
pub enum InvalidationIntakeDecisionV1 {
    ProtectedSupport(ProofCarryingInvalidationAuthorityV1),
    RetrievalOnly(CausalArtifactIntakeRecordV1),
    Rejected(CausalArtifactIntakeRecordV1),
}

pub fn verify_causal_artifact_intake_v1(
    claim: CausalArtifactIntakeClaimV1,
    support_closure: CertifiedInfluenceClosure,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<InvalidationIntakeDecisionV1, InvalidationIntakeErrorV1> {
    claim.validate()?;
    support_closure.validate().map_err(|error| {
        intake_error(
            InvalidationFailureCodeV1::InvalidSupportClosure,
            error.to_string(),
        )
    })?;
    if claim.support_closure_digest != support_closure.certificate_digest {
        return Err(intake_error(
            InvalidationFailureCodeV1::BindingMismatch,
            "causal artifact claim does not bind the supplied support closure",
        ));
    }
    if q99_verifier_identity_v1(evidence) != claim.verifier_identity_digest {
        return Err(intake_error(
            InvalidationFailureCodeV1::VerifierIdentityMismatch,
            "causal artifact verifier differs from its evidence route",
        ));
    }
    let envelope = artifact_envelope_bytes(&claim, &support_closure)?;
    verify_exact_successful_payload(&envelope, evidence).map_err(map_q99_evidence_error)?;
    let closure_bytes = support_closure.canonical_bytes().map_err(|error| {
        intake_error(
            InvalidationFailureCodeV1::InvalidSupportClosure,
            error.to_string(),
        )
    })?;
    let disposition = disposition_for_support(claim.support_class);
    let mut record = CausalArtifactIntakeRecordV1 {
        contract_version: INVALIDATION_INTAKE_CONTRACT_VERSION_V1,
        claim_digest: claim.digest()?,
        claim,
        support_closure,
        support_closure_bytes_digest: domain_digest(
            b"zerostack.causal_artifact.closure_bytes.v1\0",
            &closure_bytes,
        ),
        evidence_digest: verified_evidence_digest(evidence).map_err(map_q99_evidence_error)?,
        disposition,
        authority_digest: DigestV1::ZERO,
    };
    record.authority_digest = record.expected_authority_digest()?;
    record.validate()?;
    Ok(match disposition {
        InvalidationIntakeDispositionV1::ProtectedSupport => {
            InvalidationIntakeDecisionV1::ProtectedSupport(ProofCarryingInvalidationAuthorityV1 {
                record,
            })
        }
        InvalidationIntakeDispositionV1::RetrievalOnly => {
            InvalidationIntakeDecisionV1::RetrievalOnly(record)
        }
        InvalidationIntakeDispositionV1::Rejected => InvalidationIntakeDecisionV1::Rejected(record),
    })
}

const fn disposition_for_support(
    support: SupportCompletenessClassV1,
) -> InvalidationIntakeDispositionV1 {
    match support {
        SupportCompletenessClassV1::Exact | SupportCompletenessClassV1::SoundOverapproximation => {
            InvalidationIntakeDispositionV1::ProtectedSupport
        }
        SupportCompletenessClassV1::Heuristic => InvalidationIntakeDispositionV1::RetrievalOnly,
        SupportCompletenessClassV1::Unknown => InvalidationIntakeDispositionV1::Rejected,
    }
}

pub fn invalidation_intake_contract_manifest_v1() -> Value {
    json!({
        "authority": "ZeroStack",
        "causal_artifact_fields": [
            "artifact_digest", "artifact_owner", "producer_identity_digest",
            "declared_support_roots", "support_closure_digest", "support_class",
            "derivation_authority", "invalidation_predicate_digest",
            "protected_use_scope_digest", "verifier_scope_digest",
            "validation_cost_profile_digest",
            "recovery_route_digest", "verifier_identity_digest"
        ],
        "contract_version": INVALIDATION_INTAKE_CONTRACT_VERSION_V1,
        "linked_contracts": {
            "freshness": freshness_contract_digest_v1(),
            "q99": q99_contract_digest_v2(),
            "robust_snap": robust_snap_contract_digest_v1(),
        },
        "negative_space": [
            "essential_edge_as_support_completeness",
            "heuristic_support_as_protected_reuse",
            "unknown_fiber_as_snap_complete",
            "underapproximation_as_sound_fiber",
            "value_equality_as_support_validity",
            "support_validity_as_reuse_dominance",
            "validation_cost_profile_as_measured_dominance",
            "snap_record_as_operational_execution_authority"
        ],
        "protected_support_classes": ["exact", "sound_overapproximation"],
        "published_artifact_schema_sha256": CAUSAL_ARTIFACT_SCHEMA_SHA256_V1,
        "published_snap_schema_sha256": ROBUST_SNAP_INTAKE_SCHEMA_SHA256_V1,
        "snap_complete_representations": ["finite_exact", "conservative_superset"],
        "verifier_evidence": "successful_exact_payload_build_or_test_receipt",
    })
}

pub fn invalidation_intake_contract_digest_v1() -> DigestV1 {
    domain_digest(
        CONTRACT_DOMAIN_V1,
        canonical_json(&invalidation_intake_contract_manifest_v1()).as_bytes(),
    )
}

fn snap_envelope_bytes(
    claim: &RobustSnapIntakeClaimV1,
    certificate: &RobustSnapCertificate,
) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
    canonical_bytes(&json!({"claim": claim, "snap_certificate": certificate}))
}

fn artifact_envelope_bytes(
    claim: &CausalArtifactIntakeClaimV1,
    closure: &CertifiedInfluenceClosure,
) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
    canonical_bytes(&json!({"claim": claim, "support_closure": closure}))
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, InvalidationIntakeErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > INVALIDATION_MAX_CANONICAL_BYTES_V1 {
        return Err(intake_error(
            InvalidationFailureCodeV1::CanonicalPayloadTooLarge,
            "invalidation intake canonical payload exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, InvalidationIntakeErrorV1>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > INVALIDATION_MAX_CANONICAL_BYTES_V1 {
        return Err(intake_error(
            InvalidationFailureCodeV1::CanonicalPayloadTooLarge,
            "invalidation intake canonical payload exceeds its byte bound",
        ));
    }
    let value = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(intake_error(
            InvalidationFailureCodeV1::NonCanonicalEncoding,
            "invalidation intake bytes are not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_serialized<T: Serialize + ?Sized>(
    domain: &[u8],
    value: &T,
) -> Result<DigestV1, InvalidationIntakeErrorV1> {
    Ok(domain_digest(domain, &canonical_bytes(value)?))
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut value = Vec::with_capacity(domain.len() + bytes.len());
    value.extend_from_slice(domain);
    value.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&value))
}

fn require_nonzero(
    label: &'static str,
    values: &[DigestV1],
) -> Result<(), InvalidationIntakeErrorV1> {
    if values.contains(&DigestV1::ZERO) {
        Err(intake_error(
            InvalidationFailureCodeV1::ZeroDigest,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

fn map_q99_evidence_error(error: crate::q99::Q99ErrorV1) -> InvalidationIntakeErrorV1 {
    intake_error(
        InvalidationFailureCodeV1::InvalidVerifierEvidence,
        error.to_string(),
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationFailureCodeV1 {
    SchemaVersionMismatch,
    ZeroDigest,
    InvalidRobustSnap,
    InvalidSupportRoots,
    InvalidSupportClosure,
    SupportClassMismatch,
    BindingMismatch,
    VerifierIdentityMismatch,
    InvalidVerifierEvidence,
    RecordDigestMismatch,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidationIntakeErrorV1 {
    code: InvalidationFailureCodeV1,
    detail: String,
}

impl InvalidationIntakeErrorV1 {
    pub const fn failure_code(&self) -> InvalidationFailureCodeV1 {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for InvalidationIntakeErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalidation intake failed ({:?}): {}",
            self.code, self.detail
        )
    }
}
impl Error for InvalidationIntakeErrorV1 {}

fn intake_error(
    code: InvalidationFailureCodeV1,
    detail: impl Into<String>,
) -> InvalidationIntakeErrorV1 {
    InvalidationIntakeErrorV1 {
        code,
        detail: detail.into(),
    }
}
fn json_error(detail: String) -> InvalidationIntakeErrorV1 {
    intake_error(InvalidationFailureCodeV1::Json, detail)
}

