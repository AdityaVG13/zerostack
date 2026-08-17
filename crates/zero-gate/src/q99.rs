//! Proof-carrying Q99 causal-cache validation and receipt-generated claims.
//!
//! Semantic cache validity, provider telemetry, reasoning continuation, and
//! complete-work claims remain distinct. No token/cache observation can mint
//! quality authority, and no unlabeled percentage is representable here.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{ArtifactOwner, Sha256Digest, canonical_json, reasoning_contract_digest, sha256};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_ledger::{CausalCounterUnit, CausalWorkReceipt, causal_work_contract_digest};

use crate::invalidation::BoundCausalCacheInvalidation;

pub const Q99_CONTRACT_VERSION: u16 = 1;
pub const Q99_CONTRACT_VERSION_V2: u16 = 2;
pub const Q99_CACHE_SCHEMA_VERSION: &str = "zerostack.q99.causal_cache_component.v1";
pub const Q99_METRIC_SCHEMA_VERSION: &str = "zerostack.q99.metric_receipt.v1";
pub const Q99_TASK_PAIR_SCHEMA_VERSION: &str = "zerostack.q99.task_pair.v1";
pub const Q99_PREPARATION_SCHEMA_VERSION: &str = "zerostack.q99.preparation.v1";
pub const Q99_CLAIM_SCHEMA_VERSION: &str = "zerostack.q99.claim.v1";
pub const Q99_CACHE_SCHEMA_SHA256: &str =
    "3773a3c93fa8cb7259e079a68af5b84f76b92791904e8049fb028f2bdbb3e55d";
pub const Q99_CLAIM_SCHEMA_SHA256: &str =
    "3ba7ee269155c189eb6a2bda2bd3e7f2fe3ca0eefd818e5b89a5dcbabda65ef1";
pub const Q99_MAX_CANONICAL_BYTES: usize = 1_048_576;
pub const Q99_MAX_TASKS: usize = 65_536;
pub const Q99_COMPONENT_COUNT: usize = 9;

const CACHE_COMPONENT_DOMAIN: &[u8] = b"zerostack.q99.cache_component.v1\0";
const CACHE_ADMISSION_DOMAIN: &[u8] = b"zerostack.q99.cache_admission.v1\0";
const METRIC_RECEIPT_DOMAIN: &[u8] = b"zerostack.q99.metric_receipt.v1\0";
const TASK_PAIR_DOMAIN: &[u8] = b"zerostack.q99.task_pair.v1\0";
const PREPARATION_DOMAIN: &[u8] = b"zerostack.q99.preparation.v1\0";
const VERIFIED_WORK_DOMAIN: &[u8] = b"zerostack.q99.verified_causal_work.v1\0";
const CLAIM_DOMAIN: &[u8] = b"zerostack.q99.claim.v1\0";
const VERIFIER_DOMAIN: &[u8] = b"zerostack.q99.verifier_identity.v1\0";
const CONTRACT_DOMAIN: &[u8] = b"zerostack.q99.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCoordinate {
    Source,
    Producer,
    Graph,
    Tokenization,
    Rendering,
    ProviderCache,
    ReasoningContinuation,
    Verifier,
    Quality,
}

impl CacheCoordinate {
    pub const ALL: [Self; Q99_COMPONENT_COUNT] = [
        Self::Source,
        Self::Producer,
        Self::Graph,
        Self::Tokenization,
        Self::Rendering,
        Self::ProviderCache,
        Self::ReasoningContinuation,
        Self::Verifier,
        Self::Quality,
    ];

    const fn expected_owner(self, artifact_owner: ArtifactOwner) -> ArtifactOwner {
        match self {
            Self::Source => ArtifactOwner::FsZero,
            Self::Producer => artifact_owner,
            Self::Graph => ArtifactOwner::GraphZero,
            Self::Tokenization
            | Self::Rendering
            | Self::ProviderCache
            | Self::ReasoningContinuation => ArtifactOwner::TokenZero,
            Self::Verifier | Self::Quality => ArtifactOwner::ZeroStack,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "validity", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheValidity {
    Exact,
    SoundOverapproximation,
    ByteIdenticalPrefix,
    ProviderEligible,
    ProviderReportedHit { tokens: u64 },
    ExactReasoningContinuation,
    Approximate { error_bound_digest: Sha256Digest },
    Unknown,
    Invalid { reason_digest: Sha256Digest },
}

impl CacheValidity {
    fn validate_for(&self, coordinate: CacheCoordinate) -> Result<(), Q99Error> {
        match self {
            Self::Exact
                if matches!(
                    coordinate,
                    CacheCoordinate::ProviderCache | CacheCoordinate::ReasoningContinuation
                ) =>
            {
                Err(q99_error(
                    Q99FailureCode::StatusCoordinateMismatch,
                    "provider and reasoning coordinates require their distinct exact statuses",
                ))
            }
            Self::SoundOverapproximation if coordinate != CacheCoordinate::Graph => {
                Err(q99_error(
                    Q99FailureCode::StatusCoordinateMismatch,
                    "sound overapproximation is only a graph/dependency-closure status",
                ))
            }
            Self::ByteIdenticalPrefix if coordinate != CacheCoordinate::Rendering => {
                Err(q99_error(
                    Q99FailureCode::StatusCoordinateMismatch,
                    "byte-identical prefix is only a rendering status",
                ))
            }
            Self::ProviderEligible | Self::ProviderReportedHit { .. }
                if coordinate != CacheCoordinate::ProviderCache =>
            {
                Err(q99_error(
                    Q99FailureCode::StatusCoordinateMismatch,
                    "provider cache telemetry is only a provider-cache status",
                ))
            }
            Self::ProviderReportedHit { tokens: 0 } => Err(q99_error(
                Q99FailureCode::InvalidTelemetry,
                "provider-reported hit tokens must be nonzero",
            )),
            Self::ExactReasoningContinuation
                if coordinate != CacheCoordinate::ReasoningContinuation =>
            {
                Err(q99_error(
                    Q99FailureCode::StatusCoordinateMismatch,
                    "exact reasoning continuation is only a reasoning status",
                ))
            }
            Self::Approximate { error_bound_digest } if *error_bound_digest == Sha256Digest::ZERO => {
                Err(q99_error(
                    Q99FailureCode::ZeroDigest,
                    "approximate status requires a nonzero error-bound digest",
                ))
            }
            Self::Invalid { reason_digest } if *reason_digest == Sha256Digest::ZERO => Err(q99_error(
                Q99FailureCode::ZeroDigest,
                "invalid status requires a nonzero reason digest",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheBinding {
    pub artifact_digest: Sha256Digest,
    pub artifact_owner: ArtifactOwner,
    pub source_root: Sha256Digest,
    pub dependency_root: Sha256Digest,
    pub producer_contract_digest: Sha256Digest,
    pub protected_use_class_digest: Sha256Digest,
    pub reasoning_contract_digest: Sha256Digest,
    pub verifier_scope_digest: Sha256Digest,
    pub invalidation_certificate_digest: Sha256Digest,
    pub recovery_route_digest: Sha256Digest,
}

impl CausalCacheBinding {
    pub fn validate(&self) -> Result<(), Q99Error> {
        require_nonzero(
            "causal cache binding",
            &[
                self.artifact_digest,
                self.source_root,
                self.dependency_root,
                self.producer_contract_digest,
                self.protected_use_class_digest,
                self.reasoning_contract_digest,
                self.verifier_scope_digest,
                self.invalidation_certificate_digest,
                self.recovery_route_digest,
            ],
        )
    }

    pub fn digest(&self) -> Result<Sha256Digest, Q99Error> {
        self.validate()?;
        digest_serialized(b"zerostack.q99.cache_binding.v1\0", self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheComponentClaim {
    schema_version: String,
    binding: CausalCacheBinding,
    coordinate: CacheCoordinate,
    owner: ArtifactOwner,
    validity: CacheValidity,
    component_receipt_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
}

impl CausalCacheComponentClaim {
    pub fn new(
        binding: CausalCacheBinding,
        coordinate: CacheCoordinate,
        owner: ArtifactOwner,
        validity: CacheValidity,
        component_receipt_digest: Sha256Digest,
        verifier_identity_digest: Sha256Digest,
    ) -> Result<Self, Q99Error> {
        let claim = Self {
            schema_version: Q99_CACHE_SCHEMA_VERSION.into(),
            binding,
            coordinate,
            owner,
            validity,
            component_receipt_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), Q99Error> {
        if self.schema_version != Q99_CACHE_SCHEMA_VERSION {
            return Err(q99_error(
                Q99FailureCode::SchemaVersionMismatch,
                "cache component schema version is not v1",
            ));
        }
        self.binding.validate()?;
        require_nonzero(
            "cache component",
            &[self.component_receipt_digest, self.verifier_identity_digest],
        )?;
        if self.owner != self.coordinate.expected_owner(self.binding.artifact_owner) {
            return Err(q99_error(
                Q99FailureCode::OwnerMismatch,
                "cache coordinate is not certified by its authoritative owner",
            ));
        }
        self.validity.validate_for(self.coordinate)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99Error> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Q99Error> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<Sha256Digest, Q99Error> {
        Ok(domain_digest(
            CACHE_COMPONENT_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Debug)]
pub struct VerifiedCausalCacheComponent {
    claim: CausalCacheComponentClaim,
    claim_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
}

impl VerifiedCausalCacheComponent {
    pub fn verify(
        claim: CausalCacheComponentClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99Error> {
        claim.validate()?;
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = q99_verifier_identity(evidence);
        if verifier_identity_digest != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCode::VerifierIdentityMismatch,
                "cache component verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            claim_digest: claim.digest()?,
            evidence_digest: verified_evidence_digest(evidence)?,
            verifier_identity_digest,
            claim,
        })
    }

    pub fn record(&self) -> CausalCacheComponentRecord {
        CausalCacheComponentRecord {
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheComponentRecord {
    pub claim: CausalCacheComponentClaim,
    pub claim_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
}

impl CausalCacheComponentRecord {
    pub fn validate(&self) -> Result<(), Q99Error> {
        self.claim.validate()?;
        require_nonzero(
            "cache component record",
            &[self.evidence_digest, self.verifier_identity_digest],
        )?;
        if self.claim.digest()? != self.claim_digest
            || self.claim.verifier_identity_digest != self.verifier_identity_digest
        {
            return Err(q99_error(
                Q99FailureCode::ReceiptDigestMismatch,
                "cache component record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheAdmissionClass {
    StrictExactReuse,
    TelemetryOnly,
    ReuseProhibited,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheAssessmentRecord {
    pub contract_version: u16,
    pub binding: CausalCacheBinding,
    pub components: Vec<CausalCacheComponentRecord>,
    pub admission_class: CacheAdmissionClass,
    pub provider_eligible: bool,
    pub provider_reported_hit_tokens: Option<u64>,
    pub exact_reasoning_continuation: bool,
    pub assessment_digest: Sha256Digest,
}

impl CausalCacheAssessmentRecord {
    pub fn validate(&self) -> Result<(), Q99Error> {
        self.binding.validate()?;
        if !matches!(
            self.contract_version,
            Q99_CONTRACT_VERSION | Q99_CONTRACT_VERSION_V2
        ) || self.components.len() != Q99_COMPONENT_COUNT
        {
            return Err(q99_error(
                Q99FailureCode::IncompleteCoordinateSet,
                "cache assessment must contain every Q99 coordinate exactly once",
            ));
        }
        let mut coordinates = BTreeSet::new();
        let mut provider_eligible = false;
        let mut provider_hit = None;
        let mut exact_reasoning = false;
        for component in &self.components {
            component.validate()?;
            if component.claim.binding != self.binding
                || !coordinates.insert(component.claim.coordinate)
            {
                return Err(q99_error(
                    Q99FailureCode::BindingMismatch,
                    "cache components do not share one binding and unique coordinates",
                ));
            }
            match (&component.claim.coordinate, &component.claim.validity) {
                (CacheCoordinate::ProviderCache, CacheValidity::ProviderEligible) => {
                    provider_eligible = true;
                }
                (
                    CacheCoordinate::ProviderCache,
                    CacheValidity::ProviderReportedHit { tokens },
                ) => {
                    provider_eligible = true;
                    provider_hit = Some(*tokens);
                }
                (
                    CacheCoordinate::ReasoningContinuation,
                    CacheValidity::ExactReasoningContinuation,
                ) => exact_reasoning = true,
                _ => {}
            }
        }
        if coordinates != CacheCoordinate::ALL.into_iter().collect()
            || provider_eligible != self.provider_eligible
            || provider_hit != self.provider_reported_hit_tokens
            || exact_reasoning != self.exact_reasoning_continuation
            || classify_cache_components(&self.components) != self.admission_class
            || self.expected_digest()? != self.assessment_digest
        {
            return Err(q99_error(
                Q99FailureCode::ReceiptDigestMismatch,
                "cache assessment status, telemetry, or digest does not replay",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<Sha256Digest, Q99Error> {
        digest_serialized(
            CACHE_ADMISSION_DOMAIN,
            &json!({
                "admission_class": self.admission_class,
                "binding": self.binding,
                "components": self.components,
                "contract_version": self.contract_version,
                "exact_reasoning_continuation": self.exact_reasoning_continuation,
                "provider_eligible": self.provider_eligible,
                "provider_reported_hit_tokens": self.provider_reported_hit_tokens,
            }),
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99Error> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Q99Error> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
pub struct CausalCacheAdmission {
    record: CausalCacheAssessmentRecord,
}

impl CausalCacheAdmission {
    pub const fn record(&self) -> &CausalCacheAssessmentRecord {
        &self.record
    }

    pub const fn binding(&self) -> &CausalCacheBinding {
        &self.record.binding
    }
}

#[derive(Debug)]
pub enum CausalCacheDecision {
    StrictReuse(CausalCacheAdmission),
    TelemetryOnly(CausalCacheAssessmentRecord),
    ReuseProhibited(CausalCacheAssessmentRecord),
}

pub fn validate_causal_cache(
    components: Vec<VerifiedCausalCacheComponent>,
    invalidation: &BoundCausalCacheInvalidation,
) -> Result<CausalCacheDecision, Q99Error> {
    if components.len() != Q99_COMPONENT_COUNT {
        return Err(q99_error(
            Q99FailureCode::IncompleteCoordinateSet,
            "aggregate validation requires all nine Q99 coordinates",
        ));
    }
    let binding = components[0].claim.binding.clone();
    if !invalidation.authorizes(&binding).map_err(|error| {
        q99_error(
            Q99FailureCode::InvalidationAuthorityMismatch,
            error.to_string(),
        )
    })? {
        return Err(q99_error(
            Q99FailureCode::InvalidationAuthorityMismatch,
            "proof-carrying invalidation authority does not bind this cache line",
        ));
    }
    let mut records = components
        .into_iter()
        .map(|component| component.record())
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.claim.coordinate);
    let admission_class = classify_cache_components(&records);
    let provider = records
        .iter()
        .find(|record| record.claim.coordinate == CacheCoordinate::ProviderCache)
        .ok_or_else(|| {
            q99_error(
                Q99FailureCode::IncompleteCoordinateSet,
                "provider coordinate is absent",
            )
        })?;
    let (provider_eligible, provider_reported_hit_tokens) = match provider.claim.validity {
        CacheValidity::ProviderEligible => (true, None),
        CacheValidity::ProviderReportedHit { tokens } => (true, Some(tokens)),
        _ => (false, None),
    };
    let exact_reasoning_continuation = records.iter().any(|record| {
        record.claim.coordinate == CacheCoordinate::ReasoningContinuation
            && record.claim.validity == CacheValidity::ExactReasoningContinuation
    });
    let mut record = CausalCacheAssessmentRecord {
        contract_version: Q99_CONTRACT_VERSION_V2,
        binding,
        components: records,
        admission_class,
        provider_eligible,
        provider_reported_hit_tokens,
        exact_reasoning_continuation,
        assessment_digest: Sha256Digest::ZERO,
    };
    record.assessment_digest = record.expected_digest()?;
    record.validate()?;
    Ok(match admission_class {
        CacheAdmissionClass::StrictExactReuse => {
            CausalCacheDecision::StrictReuse(CausalCacheAdmission { record })
        }
        CacheAdmissionClass::TelemetryOnly => CausalCacheDecision::TelemetryOnly(record),
        CacheAdmissionClass::ReuseProhibited => CausalCacheDecision::ReuseProhibited(record),
    })
}

fn classify_cache_components(components: &[CausalCacheComponentRecord]) -> CacheAdmissionClass {
    let mut strict = true;
    let mut prohibited = false;
    for component in components {
        let coordinate = component.claim.coordinate;
        let status = &component.claim.validity;
        match status {
            CacheValidity::Invalid { .. } | CacheValidity::Unknown => prohibited = true,
            CacheValidity::Approximate { .. } | CacheValidity::ByteIdenticalPrefix => {
                strict = false
            }
            CacheValidity::SoundOverapproximation => {
                strict &= coordinate == CacheCoordinate::Graph;
            }
            CacheValidity::ProviderEligible | CacheValidity::ProviderReportedHit { .. } => {
                strict &= coordinate == CacheCoordinate::ProviderCache;
            }
            CacheValidity::ExactReasoningContinuation => {
                strict &= coordinate == CacheCoordinate::ReasoningContinuation;
            }
            CacheValidity::Exact => {}
        }
    }
    if prohibited {
        CacheAdmissionClass::ReuseProhibited
    } else if strict {
        CacheAdmissionClass::StrictExactReuse
    } else {
        CacheAdmissionClass::TelemetryOnly
    }
}

#[derive(Debug)]
pub struct VerifiedCausalWorkReceipt {
    receipt: CausalWorkReceipt,
    canonical_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
}

impl VerifiedCausalWorkReceipt {
    pub fn verify(
        receipt: CausalWorkReceipt,
        verifier_identity_digest: Sha256Digest,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99Error> {
        receipt.validate().map_err(|error| {
            q99_error(
                Q99FailureCode::InvalidCausalWorkReceipt,
                format!("causal-work receipt is invalid: {error}"),
            )
        })?;
        let bytes = canonical_causal_work_bytes(&receipt)?;
        verify_exact_successful_payload(&bytes, evidence)?;
        let observed_verifier = q99_verifier_identity(evidence);
        if observed_verifier != verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCode::VerifierIdentityMismatch,
                "causal-work verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            canonical_digest: domain_digest(VERIFIED_WORK_DOMAIN, &bytes),
            evidence_digest: verified_evidence_digest(evidence)?,
            verifier_identity_digest,
            receipt,
        })
    }

    fn profile(&self) -> WorkProfile {
        let identity = &self.receipt.measurement.identity;
        WorkProfile {
            counter_id: identity.counter_id.clone(),
            unit: identity.unit,
            adapter_digest: identity.adapter_digest,
            platform_profile_digest: identity.platform_profile_digest,
        }
    }

    fn total(&self) -> u64 {
        self.receipt.observed_total
    }
    fn receipt_digest(&self) -> Sha256Digest {
        self.receipt.receipt_digest
    }

    fn record(&self) -> VerifiedCausalWorkRecord {
        VerifiedCausalWorkRecord {
            receipt: self.receipt.clone(),
            canonical_digest: self.canonical_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkProfile {
    pub counter_id: String,
    pub unit: CausalCounterUnit,
    pub adapter_digest: Sha256Digest,
    pub platform_profile_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedCausalWorkRecord {
    pub receipt: CausalWorkReceipt,
    pub canonical_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
}

impl VerifiedCausalWorkRecord {
    pub fn validate(&self) -> Result<(), Q99Error> {
        self.receipt.validate().map_err(|error| {
            q99_error(
                Q99FailureCode::InvalidCausalWorkReceipt,
                error.to_string(),
            )
        })?;
        let bytes = canonical_causal_work_bytes(&self.receipt)?;
        require_nonzero(
            "verified causal work record",
            &[self.evidence_digest, self.verifier_identity_digest],
        )?;
        if domain_digest(VERIFIED_WORK_DOMAIN, &bytes) != self.canonical_digest {
            return Err(q99_error(
                Q99FailureCode::ReceiptDigestMismatch,
                "verified causal-work record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99TaskPairClaim {
    schema_version: String,
    comparison_identity_digest: Sha256Digest,
    workload_digest: Sha256Digest,
    task_digest: Sha256Digest,
    baseline_receipt_digest: Sha256Digest,
    complete_receipt_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
}

impl Q99TaskPairClaim {
    pub fn new(
        comparison_identity_digest: Sha256Digest,
        workload_digest: Sha256Digest,
        task_digest: Sha256Digest,
        baseline_receipt_digest: Sha256Digest,
        complete_receipt_digest: Sha256Digest,
        verifier_identity_digest: Sha256Digest,
    ) -> Result<Self, Q99Error> {
        let claim = Self {
            schema_version: Q99_TASK_PAIR_SCHEMA_VERSION.into(),
            comparison_identity_digest,
            workload_digest,
            task_digest,
            baseline_receipt_digest,
            complete_receipt_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), Q99Error> {
        if self.schema_version != Q99_TASK_PAIR_SCHEMA_VERSION {
            return Err(q99_error(
                Q99FailureCode::SchemaVersionMismatch,
                "Q99 task-pair schema version is not v1",
            ));
        }
        require_nonzero(
            "Q99 task pair",
            &[
                self.comparison_identity_digest,
                self.workload_digest,
                self.task_digest,
                self.baseline_receipt_digest,
                self.complete_receipt_digest,
                self.verifier_identity_digest,
            ],
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99Error> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, Q99Error> {
        Ok(domain_digest(TASK_PAIR_DOMAIN, &self.canonical_bytes()?))
    }
}

#[derive(Debug)]
pub struct VerifiedQ99TaskPair {
    claim: Q99TaskPairClaim,
    baseline: VerifiedCausalWorkReceipt,
    complete: VerifiedCausalWorkReceipt,
    pair_evidence_digest: Sha256Digest,
}

impl VerifiedQ99TaskPair {
    pub fn verify(
        claim: Q99TaskPairClaim,
        baseline: VerifiedCausalWorkReceipt,
        complete: VerifiedCausalWorkReceipt,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99Error> {
        claim.validate()?;
        if baseline.receipt_digest() != claim.baseline_receipt_digest
            || complete.receipt_digest() != claim.complete_receipt_digest
            || baseline.profile() != complete.profile()
        {
            return Err(q99_error(
                Q99FailureCode::WorkProfileMismatch,
                "paired work receipts differ from the claim or native counter profile",
            ));
        }
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        if q99_verifier_identity(evidence) != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCode::VerifierIdentityMismatch,
                "task-pair verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            claim,
            baseline,
            complete,
            pair_evidence_digest: verified_evidence_digest(evidence)?,
        })
    }

    fn record(&self) -> Result<Q99TaskPairRecord, Q99Error> {
        Ok(Q99TaskPairRecord {
            claim_digest: self.claim.digest()?,
            claim: self.claim.clone(),
            baseline: self.baseline.record(),
            complete: self.complete.record(),
            pair_evidence_digest: self.pair_evidence_digest,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99TaskPairRecord {
    pub claim_digest: Sha256Digest,
    pub claim: Q99TaskPairClaim,
    pub baseline: VerifiedCausalWorkRecord,
    pub complete: VerifiedCausalWorkRecord,
    pub pair_evidence_digest: Sha256Digest,
}

impl Q99TaskPairRecord {
    pub fn validate(&self) -> Result<(), Q99Error> {
        self.claim.validate()?;
        self.baseline.validate()?;
        self.complete.validate()?;
        require_nonzero("task pair record", &[self.pair_evidence_digest])?;
        if self.claim.digest()? != self.claim_digest
            || self.baseline.receipt.receipt_digest != self.claim.baseline_receipt_digest
            || self.complete.receipt.receipt_digest != self.claim.complete_receipt_digest
            || work_profile(&self.baseline.receipt) != work_profile(&self.complete.receipt)
        {
            return Err(q99_error(
                Q99FailureCode::ReceiptDigestMismatch,
                "task pair record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99PreparationClaim {
    schema_version: String,
    comparison_identity_digest: Sha256Digest,
    workload_digest: Sha256Digest,
    preparation_receipt_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
}

impl Q99PreparationClaim {
    pub fn new(
        comparison_identity_digest: Sha256Digest,
        workload_digest: Sha256Digest,
        preparation_receipt_digest: Sha256Digest,
        verifier_identity_digest: Sha256Digest,
    ) -> Result<Self, Q99Error> {
        let claim = Self {
            schema_version: Q99_PREPARATION_SCHEMA_VERSION.into(),
            comparison_identity_digest,
            workload_digest,
            preparation_receipt_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), Q99Error> {
        if self.schema_version != Q99_PREPARATION_SCHEMA_VERSION {
            return Err(q99_error(
                Q99FailureCode::SchemaVersionMismatch,
                "Q99 preparation schema version is not v1",
            ));
        }
        require_nonzero(
            "Q99 preparation",
            &[
                self.comparison_identity_digest,
                self.workload_digest,
                self.preparation_receipt_digest,
                self.verifier_identity_digest,
            ],
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99Error> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, Q99Error> {
        Ok(domain_digest(
            PREPARATION_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Debug)]
pub struct VerifiedQ99Preparation {
    claim: Q99PreparationClaim,
    work: VerifiedCausalWorkReceipt,
    evidence_digest: Sha256Digest,
}

impl VerifiedQ99Preparation {
    pub fn verify(
        claim: Q99PreparationClaim,
        work: VerifiedCausalWorkReceipt,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99Error> {
        claim.validate()?;
        if claim.preparation_receipt_digest != work.receipt_digest() {
            return Err(q99_error(
                Q99FailureCode::BindingMismatch,
                "preparation claim does not bind its causal-work receipt",
            ));
        }
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        if q99_verifier_identity(evidence) != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCode::VerifierIdentityMismatch,
                "preparation verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            claim,
            work,
            evidence_digest: verified_evidence_digest(evidence)?,
        })
    }

    fn record(&self) -> Result<Q99PreparationRecord, Q99Error> {
        Ok(Q99PreparationRecord {
            claim_digest: self.claim.digest()?,
            claim: self.claim.clone(),
            work: self.work.record(),
            evidence_digest: self.evidence_digest,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99PreparationRecord {
    pub claim_digest: Sha256Digest,
    pub claim: Q99PreparationClaim,
    pub work: VerifiedCausalWorkRecord,
    pub evidence_digest: Sha256Digest,
}

impl Q99PreparationRecord {
    pub fn validate(&self) -> Result<(), Q99Error> {
        self.claim.validate()?;
        self.work.validate()?;
        require_nonzero("preparation record", &[self.evidence_digest])?;
        if self.claim.digest()? != self.claim_digest
            || self.claim.preparation_receipt_digest != self.work.receipt.receipt_digest
        {
            return Err(q99_error(
                Q99FailureCode::ReceiptDigestMismatch,
                "preparation record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Q99Label {
    Q99State,
    Q99Input,
    Q99Total,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Q99ThresholdRelation {
    AtLeast99Of100,
    AtMost1Of100,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99ClaimRecord {
    pub schema_version: String,
    pub label: Q99Label,
    pub comparison_identity_digest: Sha256Digest,
    pub workload_digest: Sha256Digest,
    pub work_profile: Option<WorkProfile>,
    pub task_count: u64,
    pub observed_numerator: String,
    pub denominator: String,
    pub threshold_relation: Q99ThresholdRelation,
    pub threshold_numerator: u8,
    pub threshold_denominator: u8,
    pub attained: bool,
    pub source_receipt_digests: Vec<Sha256Digest>,
    pub claim_digest: Sha256Digest,
}

impl Q99ClaimRecord {
    pub fn validate(&self) -> Result<(), Q99Error> {
        if self.schema_version != Q99_CLAIM_SCHEMA_VERSION
            || self.task_count == 0
            || self.task_count as usize > Q99_MAX_TASKS
            || self.source_receipt_digests.is_empty()
            || self.source_receipt_digests.len() > (Q99_MAX_TASKS * 2 + 1)
            || self.source_receipt_digests.contains(&Sha256Digest::ZERO)
            || self
                .source_receipt_digests
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(q99_error(
                Q99FailureCode::InvalidClaim,
                "Q99 claim has invalid version, task count, or receipt set",
            ));
        }
        require_nonzero(
            "Q99 claim",
            &[self.comparison_identity_digest, self.workload_digest],
        )?;
        let numerator = parse_u128("observed numerator", &self.observed_numerator)?;
        let denominator = parse_u128("denominator", &self.denominator)?;
        if denominator == 0 {
            return Err(q99_error(
                Q99FailureCode::ZeroDenominator,
                "Q99 denominator cannot be zero",
            ));
        }
        if matches!(self.label, Q99Label::Q99State | Q99Label::Q99Input)
            && numerator > denominator
        {
            return Err(q99_error(
                Q99FailureCode::InvalidClaim,
                "Q99-State and Q99-Input numerators cannot exceed their denominators",
            ));
        }
        let expected = match (self.label, self.threshold_relation) {
            (
                Q99Label::Q99State | Q99Label::Q99Input,
                Q99ThresholdRelation::AtLeast99Of100,
            ) => {
                self.work_profile.is_none()
                    && self.threshold_numerator == 99
                    && self.threshold_denominator == 100
                    && checked_product(numerator, 100)? >= checked_product(denominator, 99)?
            }
            (Q99Label::Q99Total, Q99ThresholdRelation::AtMost1Of100) => {
                self.work_profile.is_some()
                    && self.threshold_numerator == 1
                    && self.threshold_denominator == 100
                    && checked_product(numerator, 100)? <= denominator
            }
            _ => false,
        };
        if expected != self.attained || self.expected_digest()? != self.claim_digest {
            return Err(q99_error(
                Q99FailureCode::InvalidClaim,
                "Q99 label, denominator, threshold, outcome, or digest does not replay",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<Sha256Digest, Q99Error> {
        digest_serialized(
            CLAIM_DOMAIN,
            &json!({
                "attained": self.attained,
                "comparison_identity_digest": self.comparison_identity_digest,
                "denominator": self.denominator,
                "label": self.label,
                "observed_numerator": self.observed_numerator,
                "schema_version": self.schema_version,
                "source_receipt_digests": self.source_receipt_digests,
                "task_count": self.task_count,
                "threshold_denominator": self.threshold_denominator,
                "threshold_numerator": self.threshold_numerator,
                "threshold_relation": self.threshold_relation,
                "work_profile": self.work_profile,
                "workload_digest": self.workload_digest,
            }),
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99Error> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Q99Error> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
pub struct Q99Certificate {
    record: Q99ClaimRecord,
}

impl Q99Certificate {
    pub const fn record(&self) -> &Q99ClaimRecord {
        &self.record
    }
}

#[derive(Debug)]
pub enum Q99ClaimDecision {
    Attained(Q99Certificate),
    NotAttained(Q99ClaimRecord),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99MetricReceiptClaim {
    schema_version: String,
    label: Q99Label,
    comparison_identity_digest: Sha256Digest,
    workload_digest: Sha256Digest,
    task_count: u64,
    observed_numerator: String,
    denominator: String,
    measurement_receipt_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
}

impl Q99MetricReceiptClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        label: Q99Label,
        comparison_identity_digest: Sha256Digest,
        workload_digest: Sha256Digest,
        task_count: u64,
        observed_numerator: u128,
        denominator: u128,
        measurement_receipt_digest: Sha256Digest,
        verifier_identity_digest: Sha256Digest,
    ) -> Result<Self, Q99Error> {
        if label == Q99Label::Q99Total {
            return Err(q99_error(
                Q99FailureCode::LabelMismatch,
                "Q99-Total can only be generated from conserved causal-work receipts",
            ));
        }
        let claim = Self {
            schema_version: Q99_METRIC_SCHEMA_VERSION.into(),
            label,
            comparison_identity_digest,
            workload_digest,
            task_count,
            observed_numerator: observed_numerator.to_string(),
            denominator: denominator.to_string(),
            measurement_receipt_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), Q99Error> {
        if self.schema_version != Q99_METRIC_SCHEMA_VERSION
            || self.label == Q99Label::Q99Total
            || self.task_count == 0
            || self.task_count as usize > Q99_MAX_TASKS
        {
            return Err(q99_error(
                Q99FailureCode::InvalidMetricReceipt,
                "Q99 metric receipt has an invalid version, label, or task count",
            ));
        }
        require_nonzero(
            "Q99 metric receipt",
            &[
                self.comparison_identity_digest,
                self.workload_digest,
                self.measurement_receipt_digest,
                self.verifier_identity_digest,
            ],
        )?;
        let numerator = parse_u128("metric numerator", &self.observed_numerator)?;
        let denominator = parse_u128("metric denominator", &self.denominator)?;
        if denominator == 0 || numerator > denominator {
            return Err(q99_error(
                Q99FailureCode::InvalidMetricReceipt,
                "Q99 metric numerator must be within a nonzero labeled denominator",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99Error> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, Q99Error> {
        Ok(domain_digest(
            METRIC_RECEIPT_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Debug)]
pub struct VerifiedQ99MetricReceipt {
    claim: Q99MetricReceiptClaim,
    receipt_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
}

impl VerifiedQ99MetricReceipt {
    pub fn verify(
        claim: Q99MetricReceiptClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99Error> {
        claim.validate()?;
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        if q99_verifier_identity(evidence) != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCode::VerifierIdentityMismatch,
                "metric receipt verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            receipt_digest: claim.digest()?,
            evidence_digest: verified_evidence_digest(evidence)?,
            claim,
        })
    }
}

pub fn generate_q99_metric_claim(
    receipt: VerifiedQ99MetricReceipt,
) -> Result<Q99ClaimDecision, Q99Error> {
    let numerator = parse_u128("metric numerator", &receipt.claim.observed_numerator)?;
    let denominator = parse_u128("metric denominator", &receipt.claim.denominator)?;
    let attained = checked_product(numerator, 100)? >= checked_product(denominator, 99)?;
    let mut record = Q99ClaimRecord {
        schema_version: Q99_CLAIM_SCHEMA_VERSION.into(),
        label: receipt.claim.label,
        comparison_identity_digest: receipt.claim.comparison_identity_digest,
        workload_digest: receipt.claim.workload_digest,
        work_profile: None,
        task_count: receipt.claim.task_count,
        observed_numerator: numerator.to_string(),
        denominator: denominator.to_string(),
        threshold_relation: Q99ThresholdRelation::AtLeast99Of100,
        threshold_numerator: 99,
        threshold_denominator: 100,
        attained,
        source_receipt_digests: sorted_unique_digests(vec![
            receipt.receipt_digest,
            receipt.evidence_digest,
            receipt.claim.measurement_receipt_digest,
        ])?,
        claim_digest: Sha256Digest::ZERO,
    };
    record.claim_digest = record.expected_digest()?;
    record.validate()?;
    Ok(if attained {
        Q99ClaimDecision::Attained(Q99Certificate { record })
    } else {
        Q99ClaimDecision::NotAttained(record)
    })
}

pub fn generate_q99_total_claim(
    preparation: VerifiedQ99Preparation,
    task_pairs: Vec<VerifiedQ99TaskPair>,
) -> Result<Q99ClaimDecision, Q99Error> {
    if task_pairs.is_empty() || task_pairs.len() > Q99_MAX_TASKS {
        return Err(q99_error(
            Q99FailureCode::InvalidTaskSet,
            "Q99-Total requires 1..=65536 paired tasks",
        ));
    }
    let comparison = preparation.claim.comparison_identity_digest;
    let workload = preparation.claim.workload_digest;
    let profile = preparation.work.profile();
    let mut tasks = BTreeSet::new();
    let mut baseline_total = 0_u128;
    let mut complete_total = u128::from(preparation.work.total());
    let mut complete_work_units = BTreeSet::new();
    let mut baseline_work_units = BTreeSet::new();
    let mut source_receipts = vec![
        preparation.claim.digest()?,
        preparation.claim.preparation_receipt_digest,
        preparation.evidence_digest,
        preparation.work.evidence_digest,
    ];
    for charge in &preparation.work.receipt.charges {
        if !complete_work_units.insert(charge.work_unit_id) {
            return Err(q99_error(
                Q99FailureCode::DuplicateWorkUnit,
                "preparation and complete work double-count a work unit",
            ));
        }
    }
    for pair in task_pairs {
        if pair.claim.comparison_identity_digest != comparison
            || pair.claim.workload_digest != workload
            || pair.baseline.profile() != profile
            || pair.complete.profile() != profile
            || !tasks.insert(pair.claim.task_digest)
        {
            return Err(q99_error(
                Q99FailureCode::WorkProfileMismatch,
                "Q99 task pairs do not share one workload, comparison, profile, and unique task",
            ));
        }
        for charge in &pair.baseline.receipt.charges {
            if !baseline_work_units.insert(charge.work_unit_id) {
                return Err(q99_error(
                    Q99FailureCode::DuplicateWorkUnit,
                    "baseline denominator double-counts a work unit",
                ));
            }
        }
        for charge in &pair.complete.receipt.charges {
            if !complete_work_units.insert(charge.work_unit_id) {
                return Err(q99_error(
                    Q99FailureCode::DuplicateWorkUnit,
                    "complete numerator double-counts a work unit",
                ));
            }
        }
        baseline_total = baseline_total
            .checked_add(u128::from(pair.baseline.total()))
            .ok_or_else(|| {
                q99_error(
                    Q99FailureCode::ArithmeticOverflow,
                    "baseline work sum overflowed",
                )
            })?;
        complete_total = complete_total
            .checked_add(u128::from(pair.complete.total()))
            .ok_or_else(|| {
                q99_error(
                    Q99FailureCode::ArithmeticOverflow,
                    "complete work sum overflowed",
                )
            })?;
        source_receipts.extend([
            pair.claim.digest()?,
            pair.claim.baseline_receipt_digest,
            pair.claim.complete_receipt_digest,
            pair.pair_evidence_digest,
            pair.baseline.evidence_digest,
            pair.complete.evidence_digest,
        ]);
        pair.record()?.validate()?;
    }
    if baseline_total == 0 {
        return Err(q99_error(
            Q99FailureCode::ZeroDenominator,
            "Q99-Total raw-baseline denominator is zero",
        ));
    }
    let attained = checked_product(complete_total, 100)? <= baseline_total;
    let mut record = Q99ClaimRecord {
        schema_version: Q99_CLAIM_SCHEMA_VERSION.into(),
        label: Q99Label::Q99Total,
        comparison_identity_digest: comparison,
        workload_digest: workload,
        work_profile: Some(profile),
        task_count: tasks.len() as u64,
        observed_numerator: complete_total.to_string(),
        denominator: baseline_total.to_string(),
        threshold_relation: Q99ThresholdRelation::AtMost1Of100,
        threshold_numerator: 1,
        threshold_denominator: 100,
        attained,
        source_receipt_digests: sorted_unique_digests(source_receipts)?,
        claim_digest: Sha256Digest::ZERO,
    };
    record.claim_digest = record.expected_digest()?;
    record.validate()?;
    preparation.record()?.validate()?;
    Ok(if attained {
        Q99ClaimDecision::Attained(Q99Certificate { record })
    } else {
        Q99ClaimDecision::NotAttained(record)
    })
}

fn q99_contract_manifest_for(version: u16, require_invalidation: bool) -> Value {
    let mut negative_space = vec![
        "component_ratio_as_total_ratio",
        "provider_eligibility_as_hit",
        "provider_hit_as_semantic_validity",
        "prefix_reuse_as_reasoning_continuation",
        "cache_or_token_metric_as_quality",
        "unknown_or_approximate_as_strict_reuse",
        "unlabeled_percentage_claim",
        "unmeasured_or_unpaired_total_denominator",
    ];
    if require_invalidation {
        negative_space.insert(1, "component_receipts_without_bound_invalidation_authority");
    }
    let mut manifest = json!({
        "cache_coordinates": CacheCoordinate::ALL,
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "claim_labels": ["Q99-State", "Q99-Input", "Q99-Total"],
        "contract_version": version,
        "economic_state_never_implies": ["semantic_validity", "quality", "reasoning_continuation"],
        "finite_q99_total": "100*(preparation+sum_complete_task_work)<=sum_raw_baseline_task_work",
        "linked_contracts": {
            "causal_work": causal_work_contract_digest(),
            "reasoning_contract": reasoning_contract_digest(),
        },
        "negative_space": negative_space,
        "proof_carrier": "zero_cert::VerifiedEvidence_successful_build_or_test_exact_payload",
        "published_cache_schema_sha256": Q99_CACHE_SCHEMA_SHA256,
        "published_claim_schema_sha256": Q99_CLAIM_SCHEMA_SHA256,
        "q99_input": "verified_avoided_raw_baseline_input_tokens_over_raw_baseline_input_tokens",
        "q99_state": "verified_exact_reused_unchanged_artifacts_over_eligible_unchanged_artifacts",
        "q99_total_charges": [
            "preparation", "candidate", "validation", "verification", "comparison",
            "guards", "rejection", "restoration", "deoptimization", "fallback", "residue"
        ],
        "resource_arithmetic": "checked_integer_native_counter_coordinates_only",
    });
    if require_invalidation && let Value::Object(fields) = &mut manifest {
        fields.insert(
            "strict_reuse_requires".into(),
            Value::String(
                "proof_carrying_invalidation_authority_bound_to_complete_cache_line".into(),
            ),
        );
    }
    manifest
}

pub fn q99_contract_manifest() -> Value {
    q99_contract_manifest_for(Q99_CONTRACT_VERSION, false)
}

pub fn q99_contract_digest() -> Sha256Digest {
    digest_value(CONTRACT_DOMAIN, &q99_contract_manifest())
}

pub fn q99_invalidation_contract_manifest() -> Value {
    q99_contract_manifest_for(Q99_CONTRACT_VERSION_V2, true)
}

pub fn q99_invalidation_contract_digest() -> Sha256Digest {
    digest_value(CONTRACT_DOMAIN, &q99_invalidation_contract_manifest())
}

fn work_profile(receipt: &CausalWorkReceipt) -> WorkProfile {
    let identity = &receipt.measurement.identity;
    WorkProfile {
        counter_id: identity.counter_id.clone(),
        unit: identity.unit,
        adapter_digest: identity.adapter_digest,
        platform_profile_digest: identity.platform_profile_digest,
    }
}

fn canonical_causal_work_bytes(receipt: &CausalWorkReceipt) -> Result<Vec<u8>, Q99Error> {
    receipt.validate().map_err(|error| {
        q99_error(
            Q99FailureCode::InvalidCausalWorkReceipt,
            error.to_string(),
        )
    })?;
    canonical_bytes(receipt)
}

fn sorted_unique_digests(mut digests: Vec<Sha256Digest>) -> Result<Vec<Sha256Digest>, Q99Error> {
    if digests.contains(&Sha256Digest::ZERO) {
        return Err(q99_error(
            Q99FailureCode::ZeroDigest,
            "source receipt set contains a zero digest",
        ));
    }
    digests.sort();
    digests.dedup();
    Ok(digests)
}

fn checked_product(value: u128, factor: u128) -> Result<u128, Q99Error> {
    value.checked_mul(factor).ok_or_else(|| {
        q99_error(
            Q99FailureCode::ArithmeticOverflow,
            "Q99 integer product overflowed",
        )
    })
}

fn parse_u128(label: &'static str, value: &str) -> Result<u128, Q99Error> {
    if value.is_empty()
        || value.starts_with('+')
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(q99_error(
            Q99FailureCode::InvalidIntegerEncoding,
            format!("{label} is not canonical unsigned decimal"),
        ));
    }
    value.parse().map_err(|_| {
        q99_error(
            Q99FailureCode::ArithmeticOverflow,
            format!("{label} exceeds u128"),
        )
    })
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Q99Error> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > Q99_MAX_CANONICAL_BYTES {
        return Err(q99_error(
            Q99FailureCode::CanonicalPayloadTooLarge,
            "Q99 canonical payload exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, Q99Error>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > Q99_MAX_CANONICAL_BYTES {
        return Err(q99_error(
            Q99FailureCode::CanonicalPayloadTooLarge,
            "Q99 canonical payload exceeds its byte bound",
        ));
    }
    let value = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(q99_error(
            Q99FailureCode::NonCanonicalEncoding,
            "Q99 bytes are not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_serialized<T: Serialize + ?Sized>(
    domain: &[u8],
    value: &T,
) -> Result<Sha256Digest, Q99Error> {
    Ok(domain_digest(domain, &canonical_bytes(value)?))
}

fn digest_value(domain: &[u8], value: &Value) -> Sha256Digest {
    domain_digest(domain, canonical_json(value).as_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> Sha256Digest {
    let mut value = Vec::with_capacity(domain.len() + bytes.len());
    value.extend_from_slice(domain);
    value.extend_from_slice(bytes);
    Sha256Digest::from_bytes(sha256(&value))
}

fn require_nonzero(label: &'static str, values: &[Sha256Digest]) -> Result<(), Q99Error> {
    if values.contains(&Sha256Digest::ZERO) {
        Err(q99_error(
            Q99FailureCode::ZeroDigest,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn verify_exact_successful_payload(
    expected: &[u8],
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), Q99Error> {
    match (evidence.query(), &evidence.certificate().completeness) {
        (Query::BuildReceipt { .. }, CompletenessWitness::BuildReceipt { exit_code: 0, .. })
        | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
        _ => {
            return Err(q99_error(
                Q99FailureCode::UnsupportedEvidenceClass,
                "Q99 authority requires a successful build or test receipt",
            ));
        }
    }
    if evidence.payload() != expected {
        return Err(q99_error(
            Q99FailureCode::EvidencePayloadMismatch,
            "Q99 evidence payload differs from exact canonical claim bytes",
        ));
    }
    Ok(())
}

pub(crate) fn q99_verifier_identity(evidence: &VerifiedEvidence<'_, '_>) -> Sha256Digest {
    let provenance = &evidence.certificate().provenance;
    digest_value(
        VERIFIER_DOMAIN,
        &json!({
            "index_id": provenance.index_id,
            "index_version": provenance.index_version,
            "operator_id": provenance.operator_id,
            "operator_version": provenance.operator_version,
            "parser_id": provenance.parser_id,
            "parser_version": provenance.parser_version,
        }),
    )
}

pub(crate) fn verified_evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Sha256Digest, Q99Error> {
    let certificate = evidence.certificate();
    let value = serde_json::to_value(json!({
        "completeness": certificate.completeness,
        "payload_sha256": Sha256Digest::from_bytes(sha256(certificate.payload.as_ref())),
        "provenance": certificate.provenance,
        "query": certificate.query,
        "span_count": certificate.spans.len(),
    }))
    .map_err(|error| json_error(error.to_string()))?;
    Ok(digest_value(
        b"zerostack.q99.verified_evidence.v1\0",
        &value,
    ))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Q99FailureCode {
    SchemaVersionMismatch,
    ZeroDigest,
    OwnerMismatch,
    StatusCoordinateMismatch,
    InvalidTelemetry,
    IncompleteCoordinateSet,
    BindingMismatch,
    InvalidationAuthorityMismatch,
    UnsupportedEvidenceClass,
    EvidencePayloadMismatch,
    VerifierIdentityMismatch,
    InvalidCausalWorkReceipt,
    WorkProfileMismatch,
    DuplicateWorkUnit,
    InvalidTaskSet,
    ZeroDenominator,
    ArithmeticOverflow,
    InvalidIntegerEncoding,
    InvalidMetricReceipt,
    LabelMismatch,
    InvalidClaim,
    ReceiptDigestMismatch,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Q99Error {
    code: Q99FailureCode,
    detail: String,
}

impl Q99Error {
    pub const fn failure_code(&self) -> Q99FailureCode {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for Q99Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Q99 validation failed ({:?}): {}",
            self.code, self.detail
        )
    }
}
impl Error for Q99Error {}

fn q99_error(code: Q99FailureCode, detail: impl Into<String>) -> Q99Error {
    Q99Error {
        code,
        detail: detail.into(),
    }
}
fn json_error(detail: String) -> Q99Error {
    q99_error(Q99FailureCode::Json, detail)
}

