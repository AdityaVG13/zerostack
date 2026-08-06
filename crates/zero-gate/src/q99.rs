//! Proof-carrying Q99 causal-cache validation and receipt-generated claims.
//!
//! Semantic cache validity, provider telemetry, reasoning continuation, and
//! complete-work claims remain distinct. No token/cache observation can mint
//! quality authority, and no unlabeled percentage is representable here.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zero_abi::{canonical_json, reasoning_contract_digest_v1, sha256, ArtifactOwnerV1, DigestV1};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_ledger::{causal_work_contract_digest_v1, CausalCounterUnitV1, CausalWorkReceiptV1};

use crate::invalidation::BoundCausalCacheInvalidationV1;

pub const Q99_CONTRACT_VERSION_V1: u16 = 1;
pub const Q99_CONTRACT_VERSION_V2: u16 = 2;
pub const Q99_CACHE_SCHEMA_VERSION_V1: &str = "zerostack.q99.causal_cache_component.v1";
pub const Q99_METRIC_SCHEMA_VERSION_V1: &str = "zerostack.q99.metric_receipt.v1";
pub const Q99_TASK_PAIR_SCHEMA_VERSION_V1: &str = "zerostack.q99.task_pair.v1";
pub const Q99_PREPARATION_SCHEMA_VERSION_V1: &str = "zerostack.q99.preparation.v1";
pub const Q99_CLAIM_SCHEMA_VERSION_V1: &str = "zerostack.q99.claim.v1";
pub const Q99_CACHE_SCHEMA_SHA256_V1: &str =
    "3773a3c93fa8cb7259e079a68af5b84f76b92791904e8049fb028f2bdbb3e55d";
pub const Q99_CLAIM_SCHEMA_SHA256_V1: &str =
    "3ba7ee269155c189eb6a2bda2bd3e7f2fe3ca0eefd818e5b89a5dcbabda65ef1";
pub const Q99_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const Q99_MAX_TASKS_V1: usize = 65_536;
pub const Q99_COMPONENT_COUNT_V1: usize = 9;

const CACHE_COMPONENT_DOMAIN_V1: &[u8] = b"zerostack.q99.cache_component.v1\0";
const CACHE_ADMISSION_DOMAIN_V1: &[u8] = b"zerostack.q99.cache_admission.v1\0";
const METRIC_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.q99.metric_receipt.v1\0";
const TASK_PAIR_DOMAIN_V1: &[u8] = b"zerostack.q99.task_pair.v1\0";
const PREPARATION_DOMAIN_V1: &[u8] = b"zerostack.q99.preparation.v1\0";
const VERIFIED_WORK_DOMAIN_V1: &[u8] = b"zerostack.q99.verified_causal_work.v1\0";
const CLAIM_DOMAIN_V1: &[u8] = b"zerostack.q99.claim.v1\0";
const VERIFIER_DOMAIN_V1: &[u8] = b"zerostack.q99.verifier_identity.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.q99.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCoordinateV1 {
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

impl CacheCoordinateV1 {
    pub const ALL: [Self; Q99_COMPONENT_COUNT_V1] = [
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

    const fn expected_owner(self, artifact_owner: ArtifactOwnerV1) -> ArtifactOwnerV1 {
        match self {
            Self::Source => ArtifactOwnerV1::FsZero,
            Self::Producer => artifact_owner,
            Self::Graph => ArtifactOwnerV1::GraphZero,
            Self::Tokenization
            | Self::Rendering
            | Self::ProviderCache
            | Self::ReasoningContinuation => ArtifactOwnerV1::TokenZero,
            Self::Verifier | Self::Quality => ArtifactOwnerV1::ZeroStack,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "validity", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheValidityV1 {
    Exact,
    SoundOverapproximation,
    ByteIdenticalPrefix,
    ProviderEligible,
    ProviderReportedHit { tokens: u64 },
    ExactReasoningContinuation,
    Approximate { error_bound_digest: DigestV1 },
    Unknown,
    Invalid { reason_digest: DigestV1 },
}

impl CacheValidityV1 {
    fn validate_for(self: &Self, coordinate: CacheCoordinateV1) -> Result<(), Q99ErrorV1> {
        match self {
            Self::Exact
                if matches!(
                    coordinate,
                    CacheCoordinateV1::ProviderCache | CacheCoordinateV1::ReasoningContinuation
                ) =>
            {
                Err(q99_error(
                    Q99FailureCodeV1::StatusCoordinateMismatch,
                    "provider and reasoning coordinates require their distinct exact statuses",
                ))
            }
            Self::SoundOverapproximation if coordinate != CacheCoordinateV1::Graph => {
                Err(q99_error(
                    Q99FailureCodeV1::StatusCoordinateMismatch,
                    "sound overapproximation is only a graph/dependency-closure status",
                ))
            }
            Self::ByteIdenticalPrefix if coordinate != CacheCoordinateV1::Rendering => {
                Err(q99_error(
                    Q99FailureCodeV1::StatusCoordinateMismatch,
                    "byte-identical prefix is only a rendering status",
                ))
            }
            Self::ProviderEligible | Self::ProviderReportedHit { .. }
                if coordinate != CacheCoordinateV1::ProviderCache =>
            {
                Err(q99_error(
                    Q99FailureCodeV1::StatusCoordinateMismatch,
                    "provider cache telemetry is only a provider-cache status",
                ))
            }
            Self::ProviderReportedHit { tokens: 0 } => Err(q99_error(
                Q99FailureCodeV1::InvalidTelemetry,
                "provider-reported hit tokens must be nonzero",
            )),
            Self::ExactReasoningContinuation
                if coordinate != CacheCoordinateV1::ReasoningContinuation =>
            {
                Err(q99_error(
                    Q99FailureCodeV1::StatusCoordinateMismatch,
                    "exact reasoning continuation is only a reasoning status",
                ))
            }
            Self::Approximate { error_bound_digest } if *error_bound_digest == DigestV1::ZERO => {
                Err(q99_error(
                    Q99FailureCodeV1::ZeroDigest,
                    "approximate status requires a nonzero error-bound digest",
                ))
            }
            Self::Invalid { reason_digest } if *reason_digest == DigestV1::ZERO => Err(q99_error(
                Q99FailureCodeV1::ZeroDigest,
                "invalid status requires a nonzero reason digest",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheBindingV1 {
    pub artifact_digest: DigestV1,
    pub artifact_owner: ArtifactOwnerV1,
    pub source_root: DigestV1,
    pub dependency_root: DigestV1,
    pub producer_contract_digest: DigestV1,
    pub protected_use_class_digest: DigestV1,
    pub reasoning_contract_digest: DigestV1,
    pub verifier_scope_digest: DigestV1,
    pub invalidation_certificate_digest: DigestV1,
    pub recovery_route_digest: DigestV1,
}

impl CausalCacheBindingV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
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

    pub fn digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        self.validate()?;
        digest_serialized(b"zerostack.q99.cache_binding.v1\0", self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheComponentClaimV1 {
    schema_version: String,
    binding: CausalCacheBindingV1,
    coordinate: CacheCoordinateV1,
    owner: ArtifactOwnerV1,
    validity: CacheValidityV1,
    component_receipt_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl CausalCacheComponentClaimV1 {
    pub fn new(
        binding: CausalCacheBindingV1,
        coordinate: CacheCoordinateV1,
        owner: ArtifactOwnerV1,
        validity: CacheValidityV1,
        component_receipt_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, Q99ErrorV1> {
        let claim = Self {
            schema_version: Q99_CACHE_SCHEMA_VERSION_V1.into(),
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

    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        if self.schema_version != Q99_CACHE_SCHEMA_VERSION_V1 {
            return Err(q99_error(
                Q99FailureCodeV1::SchemaVersionMismatch,
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
                Q99FailureCodeV1::OwnerMismatch,
                "cache coordinate is not certified by its authoritative owner",
            ));
        }
        self.validity.validate_for(self.coordinate)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99ErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Q99ErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        Ok(domain_digest(
            CACHE_COMPONENT_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Debug)]
pub struct VerifiedCausalCacheComponentV1 {
    claim: CausalCacheComponentClaimV1,
    claim_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl VerifiedCausalCacheComponentV1 {
    pub fn verify(
        claim: CausalCacheComponentClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99ErrorV1> {
        claim.validate()?;
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = q99_verifier_identity_v1(evidence);
        if verifier_identity_digest != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCodeV1::VerifierIdentityMismatch,
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

    pub fn record(&self) -> CausalCacheComponentRecordV1 {
        CausalCacheComponentRecordV1 {
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheComponentRecordV1 {
    pub claim: CausalCacheComponentClaimV1,
    pub claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
}

impl CausalCacheComponentRecordV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        self.claim.validate()?;
        require_nonzero(
            "cache component record",
            &[self.evidence_digest, self.verifier_identity_digest],
        )?;
        if self.claim.digest()? != self.claim_digest
            || self.claim.verifier_identity_digest != self.verifier_identity_digest
        {
            return Err(q99_error(
                Q99FailureCodeV1::ReceiptDigestMismatch,
                "cache component record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheAdmissionClassV1 {
    StrictExactReuse,
    TelemetryOnly,
    ReuseProhibited,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalCacheAssessmentRecordV1 {
    pub contract_version: u16,
    pub binding: CausalCacheBindingV1,
    pub components: Vec<CausalCacheComponentRecordV1>,
    pub admission_class: CacheAdmissionClassV1,
    pub provider_eligible: bool,
    pub provider_reported_hit_tokens: Option<u64>,
    pub exact_reasoning_continuation: bool,
    pub assessment_digest: DigestV1,
}

impl CausalCacheAssessmentRecordV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        self.binding.validate()?;
        if !matches!(
            self.contract_version,
            Q99_CONTRACT_VERSION_V1 | Q99_CONTRACT_VERSION_V2
        ) || self.components.len() != Q99_COMPONENT_COUNT_V1
        {
            return Err(q99_error(
                Q99FailureCodeV1::IncompleteCoordinateSet,
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
                    Q99FailureCodeV1::BindingMismatch,
                    "cache components do not share one binding and unique coordinates",
                ));
            }
            match (&component.claim.coordinate, &component.claim.validity) {
                (CacheCoordinateV1::ProviderCache, CacheValidityV1::ProviderEligible) => {
                    provider_eligible = true;
                }
                (
                    CacheCoordinateV1::ProviderCache,
                    CacheValidityV1::ProviderReportedHit { tokens },
                ) => {
                    provider_eligible = true;
                    provider_hit = Some(*tokens);
                }
                (
                    CacheCoordinateV1::ReasoningContinuation,
                    CacheValidityV1::ExactReasoningContinuation,
                ) => exact_reasoning = true,
                _ => {}
            }
        }
        if coordinates != CacheCoordinateV1::ALL.into_iter().collect()
            || provider_eligible != self.provider_eligible
            || provider_hit != self.provider_reported_hit_tokens
            || exact_reasoning != self.exact_reasoning_continuation
            || classify_cache_components(&self.components) != self.admission_class
            || self.expected_digest()? != self.assessment_digest
        {
            return Err(q99_error(
                Q99FailureCodeV1::ReceiptDigestMismatch,
                "cache assessment status, telemetry, or digest does not replay",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        digest_serialized(
            CACHE_ADMISSION_DOMAIN_V1,
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99ErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Q99ErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
pub struct CausalCacheAdmissionV1 {
    record: CausalCacheAssessmentRecordV1,
}

impl CausalCacheAdmissionV1 {
    pub const fn record(&self) -> &CausalCacheAssessmentRecordV1 {
        &self.record
    }

    pub const fn binding(&self) -> &CausalCacheBindingV1 {
        &self.record.binding
    }
}

#[derive(Debug)]
pub enum CausalCacheDecisionV1 {
    StrictReuse(CausalCacheAdmissionV1),
    TelemetryOnly(CausalCacheAssessmentRecordV1),
    ReuseProhibited(CausalCacheAssessmentRecordV1),
}

pub fn validate_causal_cache_v1(
    components: Vec<VerifiedCausalCacheComponentV1>,
    invalidation: &BoundCausalCacheInvalidationV1,
) -> Result<CausalCacheDecisionV1, Q99ErrorV1> {
    if components.len() != Q99_COMPONENT_COUNT_V1 {
        return Err(q99_error(
            Q99FailureCodeV1::IncompleteCoordinateSet,
            "aggregate validation requires all nine Q99 coordinates",
        ));
    }
    let binding = components[0].claim.binding.clone();
    if !invalidation.authorizes(&binding).map_err(|error| {
        q99_error(
            Q99FailureCodeV1::InvalidationAuthorityMismatch,
            error.to_string(),
        )
    })? {
        return Err(q99_error(
            Q99FailureCodeV1::InvalidationAuthorityMismatch,
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
        .find(|record| record.claim.coordinate == CacheCoordinateV1::ProviderCache)
        .ok_or_else(|| {
            q99_error(
                Q99FailureCodeV1::IncompleteCoordinateSet,
                "provider coordinate is absent",
            )
        })?;
    let (provider_eligible, provider_reported_hit_tokens) = match provider.claim.validity {
        CacheValidityV1::ProviderEligible => (true, None),
        CacheValidityV1::ProviderReportedHit { tokens } => (true, Some(tokens)),
        _ => (false, None),
    };
    let exact_reasoning_continuation = records.iter().any(|record| {
        record.claim.coordinate == CacheCoordinateV1::ReasoningContinuation
            && record.claim.validity == CacheValidityV1::ExactReasoningContinuation
    });
    let mut record = CausalCacheAssessmentRecordV1 {
        contract_version: Q99_CONTRACT_VERSION_V2,
        binding,
        components: records,
        admission_class,
        provider_eligible,
        provider_reported_hit_tokens,
        exact_reasoning_continuation,
        assessment_digest: DigestV1::ZERO,
    };
    record.assessment_digest = record.expected_digest()?;
    record.validate()?;
    Ok(match admission_class {
        CacheAdmissionClassV1::StrictExactReuse => {
            CausalCacheDecisionV1::StrictReuse(CausalCacheAdmissionV1 { record })
        }
        CacheAdmissionClassV1::TelemetryOnly => CausalCacheDecisionV1::TelemetryOnly(record),
        CacheAdmissionClassV1::ReuseProhibited => CausalCacheDecisionV1::ReuseProhibited(record),
    })
}

fn classify_cache_components(components: &[CausalCacheComponentRecordV1]) -> CacheAdmissionClassV1 {
    let mut strict = true;
    let mut prohibited = false;
    for component in components {
        let coordinate = component.claim.coordinate;
        let status = &component.claim.validity;
        match status {
            CacheValidityV1::Invalid { .. } | CacheValidityV1::Unknown => prohibited = true,
            CacheValidityV1::Approximate { .. } | CacheValidityV1::ByteIdenticalPrefix => {
                strict = false
            }
            CacheValidityV1::SoundOverapproximation => {
                strict &= coordinate == CacheCoordinateV1::Graph;
            }
            CacheValidityV1::ProviderEligible | CacheValidityV1::ProviderReportedHit { .. } => {
                strict &= coordinate == CacheCoordinateV1::ProviderCache;
            }
            CacheValidityV1::ExactReasoningContinuation => {
                strict &= coordinate == CacheCoordinateV1::ReasoningContinuation;
            }
            CacheValidityV1::Exact => {}
        }
    }
    if prohibited {
        CacheAdmissionClassV1::ReuseProhibited
    } else if strict {
        CacheAdmissionClassV1::StrictExactReuse
    } else {
        CacheAdmissionClassV1::TelemetryOnly
    }
}

#[derive(Debug)]
pub struct VerifiedCausalWorkReceiptV1 {
    receipt: CausalWorkReceiptV1,
    canonical_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl VerifiedCausalWorkReceiptV1 {
    pub fn verify(
        receipt: CausalWorkReceiptV1,
        verifier_identity_digest: DigestV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99ErrorV1> {
        receipt.validate().map_err(|error| {
            q99_error(
                Q99FailureCodeV1::InvalidCausalWorkReceipt,
                format!("causal-work receipt is invalid: {error}"),
            )
        })?;
        let bytes = canonical_causal_work_bytes(&receipt)?;
        verify_exact_successful_payload(&bytes, evidence)?;
        let observed_verifier = q99_verifier_identity_v1(evidence);
        if observed_verifier != verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCodeV1::VerifierIdentityMismatch,
                "causal-work verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            canonical_digest: domain_digest(VERIFIED_WORK_DOMAIN_V1, &bytes),
            evidence_digest: verified_evidence_digest(evidence)?,
            verifier_identity_digest,
            receipt,
        })
    }

    fn profile(&self) -> WorkProfileV1 {
        let identity = &self.receipt.measurement.identity;
        WorkProfileV1 {
            counter_id: identity.counter_id.clone(),
            unit: identity.unit,
            adapter_digest: identity.adapter_digest,
            platform_profile_digest: identity.platform_profile_digest,
        }
    }

    fn total(&self) -> u64 {
        self.receipt.observed_total
    }
    fn receipt_digest(&self) -> DigestV1 {
        self.receipt.receipt_digest
    }

    fn record(&self) -> VerifiedCausalWorkRecordV1 {
        VerifiedCausalWorkRecordV1 {
            receipt: self.receipt.clone(),
            canonical_digest: self.canonical_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkProfileV1 {
    pub counter_id: String,
    pub unit: CausalCounterUnitV1,
    pub adapter_digest: DigestV1,
    pub platform_profile_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedCausalWorkRecordV1 {
    pub receipt: CausalWorkReceiptV1,
    pub canonical_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
}

impl VerifiedCausalWorkRecordV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        self.receipt.validate().map_err(|error| {
            q99_error(
                Q99FailureCodeV1::InvalidCausalWorkReceipt,
                error.to_string(),
            )
        })?;
        let bytes = canonical_causal_work_bytes(&self.receipt)?;
        require_nonzero(
            "verified causal work record",
            &[self.evidence_digest, self.verifier_identity_digest],
        )?;
        if domain_digest(VERIFIED_WORK_DOMAIN_V1, &bytes) != self.canonical_digest {
            return Err(q99_error(
                Q99FailureCodeV1::ReceiptDigestMismatch,
                "verified causal-work record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99TaskPairClaimV1 {
    schema_version: String,
    comparison_identity_digest: DigestV1,
    workload_digest: DigestV1,
    task_digest: DigestV1,
    baseline_receipt_digest: DigestV1,
    complete_receipt_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl Q99TaskPairClaimV1 {
    pub fn new(
        comparison_identity_digest: DigestV1,
        workload_digest: DigestV1,
        task_digest: DigestV1,
        baseline_receipt_digest: DigestV1,
        complete_receipt_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, Q99ErrorV1> {
        let claim = Self {
            schema_version: Q99_TASK_PAIR_SCHEMA_VERSION_V1.into(),
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

    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        if self.schema_version != Q99_TASK_PAIR_SCHEMA_VERSION_V1 {
            return Err(q99_error(
                Q99FailureCodeV1::SchemaVersionMismatch,
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99ErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        Ok(domain_digest(TASK_PAIR_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

#[derive(Debug)]
pub struct VerifiedQ99TaskPairV1 {
    claim: Q99TaskPairClaimV1,
    baseline: VerifiedCausalWorkReceiptV1,
    complete: VerifiedCausalWorkReceiptV1,
    pair_evidence_digest: DigestV1,
}

impl VerifiedQ99TaskPairV1 {
    pub fn verify(
        claim: Q99TaskPairClaimV1,
        baseline: VerifiedCausalWorkReceiptV1,
        complete: VerifiedCausalWorkReceiptV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99ErrorV1> {
        claim.validate()?;
        if baseline.receipt_digest() != claim.baseline_receipt_digest
            || complete.receipt_digest() != claim.complete_receipt_digest
            || baseline.profile() != complete.profile()
        {
            return Err(q99_error(
                Q99FailureCodeV1::WorkProfileMismatch,
                "paired work receipts differ from the claim or native counter profile",
            ));
        }
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        if q99_verifier_identity_v1(evidence) != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCodeV1::VerifierIdentityMismatch,
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

    fn record(&self) -> Result<Q99TaskPairRecordV1, Q99ErrorV1> {
        Ok(Q99TaskPairRecordV1 {
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
pub struct Q99TaskPairRecordV1 {
    pub claim_digest: DigestV1,
    pub claim: Q99TaskPairClaimV1,
    pub baseline: VerifiedCausalWorkRecordV1,
    pub complete: VerifiedCausalWorkRecordV1,
    pub pair_evidence_digest: DigestV1,
}

impl Q99TaskPairRecordV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
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
                Q99FailureCodeV1::ReceiptDigestMismatch,
                "task pair record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99PreparationClaimV1 {
    schema_version: String,
    comparison_identity_digest: DigestV1,
    workload_digest: DigestV1,
    preparation_receipt_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl Q99PreparationClaimV1 {
    pub fn new(
        comparison_identity_digest: DigestV1,
        workload_digest: DigestV1,
        preparation_receipt_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, Q99ErrorV1> {
        let claim = Self {
            schema_version: Q99_PREPARATION_SCHEMA_VERSION_V1.into(),
            comparison_identity_digest,
            workload_digest,
            preparation_receipt_digest,
            verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        if self.schema_version != Q99_PREPARATION_SCHEMA_VERSION_V1 {
            return Err(q99_error(
                Q99FailureCodeV1::SchemaVersionMismatch,
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99ErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        Ok(domain_digest(
            PREPARATION_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Debug)]
pub struct VerifiedQ99PreparationV1 {
    claim: Q99PreparationClaimV1,
    work: VerifiedCausalWorkReceiptV1,
    evidence_digest: DigestV1,
}

impl VerifiedQ99PreparationV1 {
    pub fn verify(
        claim: Q99PreparationClaimV1,
        work: VerifiedCausalWorkReceiptV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99ErrorV1> {
        claim.validate()?;
        if claim.preparation_receipt_digest != work.receipt_digest() {
            return Err(q99_error(
                Q99FailureCodeV1::BindingMismatch,
                "preparation claim does not bind its causal-work receipt",
            ));
        }
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        if q99_verifier_identity_v1(evidence) != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCodeV1::VerifierIdentityMismatch,
                "preparation verifier differs from its evidence route",
            ));
        }
        Ok(Self {
            claim,
            work,
            evidence_digest: verified_evidence_digest(evidence)?,
        })
    }

    fn record(&self) -> Result<Q99PreparationRecordV1, Q99ErrorV1> {
        Ok(Q99PreparationRecordV1 {
            claim_digest: self.claim.digest()?,
            claim: self.claim.clone(),
            work: self.work.record(),
            evidence_digest: self.evidence_digest,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99PreparationRecordV1 {
    pub claim_digest: DigestV1,
    pub claim: Q99PreparationClaimV1,
    pub work: VerifiedCausalWorkRecordV1,
    pub evidence_digest: DigestV1,
}

impl Q99PreparationRecordV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        self.claim.validate()?;
        self.work.validate()?;
        require_nonzero("preparation record", &[self.evidence_digest])?;
        if self.claim.digest()? != self.claim_digest
            || self.claim.preparation_receipt_digest != self.work.receipt.receipt_digest
        {
            return Err(q99_error(
                Q99FailureCodeV1::ReceiptDigestMismatch,
                "preparation record does not replay",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Q99LabelV1 {
    Q99State,
    Q99Input,
    Q99Total,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Q99ThresholdRelationV1 {
    AtLeast99Of100,
    AtMost1Of100,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99ClaimRecordV1 {
    pub schema_version: String,
    pub label: Q99LabelV1,
    pub comparison_identity_digest: DigestV1,
    pub workload_digest: DigestV1,
    pub work_profile: Option<WorkProfileV1>,
    pub task_count: u64,
    pub observed_numerator: String,
    pub denominator: String,
    pub threshold_relation: Q99ThresholdRelationV1,
    pub threshold_numerator: u8,
    pub threshold_denominator: u8,
    pub attained: bool,
    pub source_receipt_digests: Vec<DigestV1>,
    pub claim_digest: DigestV1,
}

impl Q99ClaimRecordV1 {
    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        if self.schema_version != Q99_CLAIM_SCHEMA_VERSION_V1
            || self.task_count == 0
            || self.task_count as usize > Q99_MAX_TASKS_V1
            || self.source_receipt_digests.is_empty()
            || self.source_receipt_digests.len() > (Q99_MAX_TASKS_V1 * 2 + 1)
            || self
                .source_receipt_digests
                .iter()
                .any(|digest| *digest == DigestV1::ZERO)
            || self
                .source_receipt_digests
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(q99_error(
                Q99FailureCodeV1::InvalidClaim,
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
                Q99FailureCodeV1::ZeroDenominator,
                "Q99 denominator cannot be zero",
            ));
        }
        if matches!(self.label, Q99LabelV1::Q99State | Q99LabelV1::Q99Input)
            && numerator > denominator
        {
            return Err(q99_error(
                Q99FailureCodeV1::InvalidClaim,
                "Q99-State and Q99-Input numerators cannot exceed their denominators",
            ));
        }
        let expected = match (self.label, self.threshold_relation) {
            (
                Q99LabelV1::Q99State | Q99LabelV1::Q99Input,
                Q99ThresholdRelationV1::AtLeast99Of100,
            ) => {
                self.work_profile.is_none()
                    && self.threshold_numerator == 99
                    && self.threshold_denominator == 100
                    && checked_product(numerator, 100)? >= checked_product(denominator, 99)?
            }
            (Q99LabelV1::Q99Total, Q99ThresholdRelationV1::AtMost1Of100) => {
                self.work_profile.is_some()
                    && self.threshold_numerator == 1
                    && self.threshold_denominator == 100
                    && checked_product(numerator, 100)? <= denominator
            }
            _ => false,
        };
        if expected != self.attained || self.expected_digest()? != self.claim_digest {
            return Err(q99_error(
                Q99FailureCodeV1::InvalidClaim,
                "Q99 label, denominator, threshold, outcome, or digest does not replay",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        digest_serialized(
            CLAIM_DOMAIN_V1,
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99ErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Q99ErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
pub struct Q99CertificateV1 {
    record: Q99ClaimRecordV1,
}

impl Q99CertificateV1 {
    pub const fn record(&self) -> &Q99ClaimRecordV1 {
        &self.record
    }
}

#[derive(Debug)]
pub enum Q99ClaimDecisionV1 {
    Attained(Q99CertificateV1),
    NotAttained(Q99ClaimRecordV1),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99MetricReceiptClaimV1 {
    schema_version: String,
    label: Q99LabelV1,
    comparison_identity_digest: DigestV1,
    workload_digest: DigestV1,
    task_count: u64,
    observed_numerator: String,
    denominator: String,
    measurement_receipt_digest: DigestV1,
    verifier_identity_digest: DigestV1,
}

impl Q99MetricReceiptClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        label: Q99LabelV1,
        comparison_identity_digest: DigestV1,
        workload_digest: DigestV1,
        task_count: u64,
        observed_numerator: u128,
        denominator: u128,
        measurement_receipt_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, Q99ErrorV1> {
        if label == Q99LabelV1::Q99Total {
            return Err(q99_error(
                Q99FailureCodeV1::LabelMismatch,
                "Q99-Total can only be generated from conserved causal-work receipts",
            ));
        }
        let claim = Self {
            schema_version: Q99_METRIC_SCHEMA_VERSION_V1.into(),
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

    pub fn validate(&self) -> Result<(), Q99ErrorV1> {
        if self.schema_version != Q99_METRIC_SCHEMA_VERSION_V1
            || self.label == Q99LabelV1::Q99Total
            || self.task_count == 0
            || self.task_count as usize > Q99_MAX_TASKS_V1
        {
            return Err(q99_error(
                Q99FailureCodeV1::InvalidMetricReceipt,
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
                Q99FailureCodeV1::InvalidMetricReceipt,
                "Q99 metric numerator must be within a nonzero labeled denominator",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Q99ErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, Q99ErrorV1> {
        Ok(domain_digest(
            METRIC_RECEIPT_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Debug)]
pub struct VerifiedQ99MetricReceiptV1 {
    claim: Q99MetricReceiptClaimV1,
    receipt_digest: DigestV1,
    evidence_digest: DigestV1,
}

impl VerifiedQ99MetricReceiptV1 {
    pub fn verify(
        claim: Q99MetricReceiptClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, Q99ErrorV1> {
        claim.validate()?;
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        if q99_verifier_identity_v1(evidence) != claim.verifier_identity_digest {
            return Err(q99_error(
                Q99FailureCodeV1::VerifierIdentityMismatch,
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

pub fn generate_q99_metric_claim_v1(
    receipt: VerifiedQ99MetricReceiptV1,
) -> Result<Q99ClaimDecisionV1, Q99ErrorV1> {
    let numerator = parse_u128("metric numerator", &receipt.claim.observed_numerator)?;
    let denominator = parse_u128("metric denominator", &receipt.claim.denominator)?;
    let attained = checked_product(numerator, 100)? >= checked_product(denominator, 99)?;
    let mut record = Q99ClaimRecordV1 {
        schema_version: Q99_CLAIM_SCHEMA_VERSION_V1.into(),
        label: receipt.claim.label,
        comparison_identity_digest: receipt.claim.comparison_identity_digest,
        workload_digest: receipt.claim.workload_digest,
        work_profile: None,
        task_count: receipt.claim.task_count,
        observed_numerator: numerator.to_string(),
        denominator: denominator.to_string(),
        threshold_relation: Q99ThresholdRelationV1::AtLeast99Of100,
        threshold_numerator: 99,
        threshold_denominator: 100,
        attained,
        source_receipt_digests: sorted_unique_digests(vec![
            receipt.receipt_digest,
            receipt.evidence_digest,
            receipt.claim.measurement_receipt_digest,
        ])?,
        claim_digest: DigestV1::ZERO,
    };
    record.claim_digest = record.expected_digest()?;
    record.validate()?;
    Ok(if attained {
        Q99ClaimDecisionV1::Attained(Q99CertificateV1 { record })
    } else {
        Q99ClaimDecisionV1::NotAttained(record)
    })
}

pub fn generate_q99_total_claim_v1(
    preparation: VerifiedQ99PreparationV1,
    task_pairs: Vec<VerifiedQ99TaskPairV1>,
) -> Result<Q99ClaimDecisionV1, Q99ErrorV1> {
    if task_pairs.is_empty() || task_pairs.len() > Q99_MAX_TASKS_V1 {
        return Err(q99_error(
            Q99FailureCodeV1::InvalidTaskSet,
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
                Q99FailureCodeV1::DuplicateWorkUnit,
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
                Q99FailureCodeV1::WorkProfileMismatch,
                "Q99 task pairs do not share one workload, comparison, profile, and unique task",
            ));
        }
        for charge in &pair.baseline.receipt.charges {
            if !baseline_work_units.insert(charge.work_unit_id) {
                return Err(q99_error(
                    Q99FailureCodeV1::DuplicateWorkUnit,
                    "baseline denominator double-counts a work unit",
                ));
            }
        }
        for charge in &pair.complete.receipt.charges {
            if !complete_work_units.insert(charge.work_unit_id) {
                return Err(q99_error(
                    Q99FailureCodeV1::DuplicateWorkUnit,
                    "complete numerator double-counts a work unit",
                ));
            }
        }
        baseline_total = baseline_total
            .checked_add(u128::from(pair.baseline.total()))
            .ok_or_else(|| {
                q99_error(
                    Q99FailureCodeV1::ArithmeticOverflow,
                    "baseline work sum overflowed",
                )
            })?;
        complete_total = complete_total
            .checked_add(u128::from(pair.complete.total()))
            .ok_or_else(|| {
                q99_error(
                    Q99FailureCodeV1::ArithmeticOverflow,
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
            Q99FailureCodeV1::ZeroDenominator,
            "Q99-Total raw-baseline denominator is zero",
        ));
    }
    let attained = checked_product(complete_total, 100)? <= baseline_total;
    let mut record = Q99ClaimRecordV1 {
        schema_version: Q99_CLAIM_SCHEMA_VERSION_V1.into(),
        label: Q99LabelV1::Q99Total,
        comparison_identity_digest: comparison,
        workload_digest: workload,
        work_profile: Some(profile),
        task_count: tasks.len() as u64,
        observed_numerator: complete_total.to_string(),
        denominator: baseline_total.to_string(),
        threshold_relation: Q99ThresholdRelationV1::AtMost1Of100,
        threshold_numerator: 1,
        threshold_denominator: 100,
        attained,
        source_receipt_digests: sorted_unique_digests(source_receipts)?,
        claim_digest: DigestV1::ZERO,
    };
    record.claim_digest = record.expected_digest()?;
    record.validate()?;
    preparation.record()?.validate()?;
    Ok(if attained {
        Q99ClaimDecisionV1::Attained(Q99CertificateV1 { record })
    } else {
        Q99ClaimDecisionV1::NotAttained(record)
    })
}

fn q99_contract_manifest(version: u16, require_invalidation: bool) -> Value {
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
        "cache_coordinates": CacheCoordinateV1::ALL,
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "claim_labels": ["Q99-State", "Q99-Input", "Q99-Total"],
        "contract_version": version,
        "economic_state_never_implies": ["semantic_validity", "quality", "reasoning_continuation"],
        "finite_q99_total": "100*(preparation+sum_complete_task_work)<=sum_raw_baseline_task_work",
        "linked_contracts": {
            "causal_work": causal_work_contract_digest_v1(),
            "reasoning_contract": reasoning_contract_digest_v1(),
        },
        "negative_space": negative_space,
        "proof_carrier": "zero_cert::VerifiedEvidence_successful_build_or_test_exact_payload",
        "published_cache_schema_sha256": Q99_CACHE_SCHEMA_SHA256_V1,
        "published_claim_schema_sha256": Q99_CLAIM_SCHEMA_SHA256_V1,
        "q99_input": "verified_avoided_raw_baseline_input_tokens_over_raw_baseline_input_tokens",
        "q99_state": "verified_exact_reused_unchanged_artifacts_over_eligible_unchanged_artifacts",
        "q99_total_charges": [
            "preparation", "candidate", "validation", "verification", "comparison",
            "guards", "rejection", "restoration", "deoptimization", "fallback", "residue"
        ],
        "resource_arithmetic": "checked_integer_native_counter_coordinates_only",
    });
    if require_invalidation {
        if let Value::Object(fields) = &mut manifest {
            fields.insert(
                "strict_reuse_requires".into(),
                Value::String(
                    "proof_carrying_invalidation_authority_bound_to_complete_cache_line".into(),
                ),
            );
        }
    }
    manifest
}

pub fn q99_contract_manifest_v1() -> Value {
    q99_contract_manifest(Q99_CONTRACT_VERSION_V1, false)
}

pub fn q99_contract_digest_v1() -> DigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &q99_contract_manifest_v1())
}

pub fn q99_contract_manifest_v2() -> Value {
    q99_contract_manifest(Q99_CONTRACT_VERSION_V2, true)
}

pub fn q99_contract_digest_v2() -> DigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &q99_contract_manifest_v2())
}

fn work_profile(receipt: &CausalWorkReceiptV1) -> WorkProfileV1 {
    let identity = &receipt.measurement.identity;
    WorkProfileV1 {
        counter_id: identity.counter_id.clone(),
        unit: identity.unit,
        adapter_digest: identity.adapter_digest,
        platform_profile_digest: identity.platform_profile_digest,
    }
}

fn canonical_causal_work_bytes(receipt: &CausalWorkReceiptV1) -> Result<Vec<u8>, Q99ErrorV1> {
    receipt.validate().map_err(|error| {
        q99_error(
            Q99FailureCodeV1::InvalidCausalWorkReceipt,
            error.to_string(),
        )
    })?;
    canonical_bytes(receipt)
}

fn sorted_unique_digests(mut digests: Vec<DigestV1>) -> Result<Vec<DigestV1>, Q99ErrorV1> {
    if digests.iter().any(|digest| *digest == DigestV1::ZERO) {
        return Err(q99_error(
            Q99FailureCodeV1::ZeroDigest,
            "source receipt set contains a zero digest",
        ));
    }
    digests.sort();
    digests.dedup();
    Ok(digests)
}

fn checked_product(value: u128, factor: u128) -> Result<u128, Q99ErrorV1> {
    value.checked_mul(factor).ok_or_else(|| {
        q99_error(
            Q99FailureCodeV1::ArithmeticOverflow,
            "Q99 integer product overflowed",
        )
    })
}

fn parse_u128(label: &'static str, value: &str) -> Result<u128, Q99ErrorV1> {
    if value.is_empty()
        || value.starts_with('+')
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(q99_error(
            Q99FailureCodeV1::InvalidIntegerEncoding,
            format!("{label} is not canonical unsigned decimal"),
        ));
    }
    value.parse().map_err(|_| {
        q99_error(
            Q99FailureCodeV1::ArithmeticOverflow,
            format!("{label} exceeds u128"),
        )
    })
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Q99ErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > Q99_MAX_CANONICAL_BYTES_V1 {
        return Err(q99_error(
            Q99FailureCodeV1::CanonicalPayloadTooLarge,
            "Q99 canonical payload exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, Q99ErrorV1>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > Q99_MAX_CANONICAL_BYTES_V1 {
        return Err(q99_error(
            Q99FailureCodeV1::CanonicalPayloadTooLarge,
            "Q99 canonical payload exceeds its byte bound",
        ));
    }
    let value = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(q99_error(
            Q99FailureCodeV1::NonCanonicalEncoding,
            "Q99 bytes are not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_serialized<T: Serialize + ?Sized>(
    domain: &[u8],
    value: &T,
) -> Result<DigestV1, Q99ErrorV1> {
    Ok(domain_digest(domain, &canonical_bytes(value)?))
}

fn digest_value(domain: &[u8], value: &Value) -> DigestV1 {
    domain_digest(domain, canonical_json(value).as_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut value = Vec::with_capacity(domain.len() + bytes.len());
    value.extend_from_slice(domain);
    value.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&value))
}

fn require_nonzero(label: &'static str, values: &[DigestV1]) -> Result<(), Q99ErrorV1> {
    if values.iter().any(|value| *value == DigestV1::ZERO) {
        Err(q99_error(
            Q99FailureCodeV1::ZeroDigest,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn verify_exact_successful_payload(
    expected: &[u8],
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), Q99ErrorV1> {
    match (evidence.query(), &evidence.certificate().completeness) {
        (Query::BuildReceipt { .. }, CompletenessWitness::BuildReceipt { exit_code: 0, .. })
        | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
        _ => {
            return Err(q99_error(
                Q99FailureCodeV1::UnsupportedEvidenceClass,
                "Q99 authority requires a successful build or test receipt",
            ));
        }
    }
    if evidence.payload() != expected {
        return Err(q99_error(
            Q99FailureCodeV1::EvidencePayloadMismatch,
            "Q99 evidence payload differs from exact canonical claim bytes",
        ));
    }
    Ok(())
}

pub(crate) fn q99_verifier_identity_v1(evidence: &VerifiedEvidence<'_, '_>) -> DigestV1 {
    let provenance = &evidence.certificate().provenance;
    digest_value(
        VERIFIER_DOMAIN_V1,
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
) -> Result<DigestV1, Q99ErrorV1> {
    let certificate = evidence.certificate();
    let value = serde_json::to_value(json!({
        "completeness": certificate.completeness,
        "payload_sha256": DigestV1::from_bytes(sha256(certificate.payload.as_ref())),
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
pub enum Q99FailureCodeV1 {
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
pub struct Q99ErrorV1 {
    code: Q99FailureCodeV1,
    detail: String,
}

impl Q99ErrorV1 {
    pub const fn failure_code(&self) -> Q99FailureCodeV1 {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for Q99ErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Q99 validation failed ({:?}): {}",
            self.code, self.detail
        )
    }
}
impl Error for Q99ErrorV1 {}

fn q99_error(code: Q99FailureCodeV1, detail: impl Into<String>) -> Q99ErrorV1 {
    Q99ErrorV1 {
        code,
        detail: detail.into(),
    }
}
fn json_error(detail: String) -> Q99ErrorV1 {
    q99_error(Q99FailureCodeV1::Json, detail)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use zero_cert::{
        verify, EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId,
    };
    use zero_ledger::{
        CausalWorkChargeV1, CausalWorkClassV1, CausalWorkOutcomeV1, ParentCounterIdentityV1,
        ParentCounterObservationV1, ParentCounterWindowV1, ResiduePolicyV1,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn verifier_route() -> DigestV1 {
        digest_value(
            VERIFIER_DOMAIN_V1,
            &json!({
                "index_id": "q99-index",
                "index_version": "1",
                "operator_id": "q99-verifier",
                "operator_version": "1",
                "parser_id": "q99-parser",
                "parser_version": "1",
            }),
        )
    }

    struct TestResolver {
        bytes: Vec<u8>,
    }
    impl Resolver for TestResolver {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == sha256(&self.bytes)).then_some(self.bytes.as_slice())
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "q99-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "q99-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "q99-index").then_some("1")
        }
    }

    fn certificate(bytes: Vec<u8>) -> (EvidenceCertificate<'static>, TestResolver) {
        let digest = sha256(&bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(99) },
                spans: vec![span],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "q99-parser".into(),
                    parser_version: "1".into(),
                    index_id: "q99-index".into(),
                    index_version: "1".into(),
                    operator_id: "q99-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "q99-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(99),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn binding() -> CausalCacheBindingV1 {
        CausalCacheBindingV1 {
            artifact_digest: d(1),
            artifact_owner: ArtifactOwnerV1::FsZero,
            source_root: d(2),
            dependency_root: d(3),
            producer_contract_digest: d(4),
            protected_use_class_digest: d(5),
            reasoning_contract_digest: d(6),
            verifier_scope_digest: d(7),
            invalidation_certificate_digest: d(8),
            recovery_route_digest: d(9),
        }
    }

    fn bound_invalidation() -> BoundCausalCacheInvalidationV1 {
        BoundCausalCacheInvalidationV1::test_only(&binding())
    }

    fn component_specs() -> Vec<(CacheCoordinateV1, ArtifactOwnerV1, CacheValidityV1)> {
        vec![
            (
                CacheCoordinateV1::Source,
                ArtifactOwnerV1::FsZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Producer,
                ArtifactOwnerV1::FsZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Graph,
                ArtifactOwnerV1::GraphZero,
                CacheValidityV1::SoundOverapproximation,
            ),
            (
                CacheCoordinateV1::Tokenization,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Rendering,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::ProviderCache,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::ProviderReportedHit { tokens: 123 },
            ),
            (
                CacheCoordinateV1::ReasoningContinuation,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::ExactReasoningContinuation,
            ),
            (
                CacheCoordinateV1::Verifier,
                ArtifactOwnerV1::ZeroStack,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Quality,
                ArtifactOwnerV1::ZeroStack,
                CacheValidityV1::Exact,
            ),
        ]
    }

    fn verified_components(
        override_status: Option<(CacheCoordinateV1, CacheValidityV1)>,
    ) -> Vec<VerifiedCausalCacheComponentV1> {
        component_specs()
            .into_iter()
            .enumerate()
            .map(|(index, (coordinate, owner, mut validity))| {
                if let Some((target, status)) = &override_status {
                    if *target == coordinate {
                        validity = status.clone();
                    }
                }
                let claim = CausalCacheComponentClaimV1::new(
                    binding(),
                    coordinate,
                    owner,
                    validity,
                    d(20 + index as u8),
                    verifier_route(),
                )
                .unwrap();
                let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
                let evidence = verify(&certificate, &resolver).unwrap();
                VerifiedCausalCacheComponentV1::verify(claim, &evidence).unwrap()
            })
            .collect()
    }

    #[test]
    fn aggregate_cache_validation_keeps_semantics_telemetry_and_reasoning_distinct() {
        let CausalCacheDecisionV1::StrictReuse(admission) =
            validate_causal_cache_v1(verified_components(None), &bound_invalidation()).unwrap()
        else {
            panic!("complete exact coordinates must admit strict reuse")
        };
        assert_eq!(admission.record().contract_version, Q99_CONTRACT_VERSION_V2);
        assert_eq!(admission.record().provider_reported_hit_tokens, Some(123));
        assert!(admission.record().provider_eligible);
        assert!(admission.record().exact_reasoning_continuation);
        let component_value =
            serde_json::to_value(&admission.record().components[0].claim).unwrap();
        let component_keys = component_value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_component_keys = [
            "schema_version",
            "binding",
            "coordinate",
            "owner",
            "validity",
            "component_receipt_digest",
            "verifier_identity_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(component_keys, expected_component_keys);
        assert_eq!(component_value["binding"]["artifact_owner"], "fs_zero");
        admission.record().validate().unwrap();
        let bytes = admission.record().canonical_bytes().unwrap();
        assert_eq!(
            CausalCacheAssessmentRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *admission.record()
        );

        let CausalCacheDecisionV1::TelemetryOnly(prefix) = validate_causal_cache_v1(
            verified_components(Some((
                CacheCoordinateV1::Rendering,
                CacheValidityV1::ByteIdenticalPrefix,
            ))),
            &bound_invalidation(),
        )
        .unwrap() else {
            panic!("prefix reuse is telemetry, not exact semantic reuse")
        };
        assert_eq!(prefix.admission_class, CacheAdmissionClassV1::TelemetryOnly);

        let CausalCacheDecisionV1::ReuseProhibited(unknown) = validate_causal_cache_v1(
            verified_components(Some((CacheCoordinateV1::Quality, CacheValidityV1::Unknown))),
            &bound_invalidation(),
        )
        .unwrap() else {
            panic!("Unknown must fail closed")
        };
        assert_eq!(
            unknown.admission_class,
            CacheAdmissionClassV1::ReuseProhibited
        );
    }

    #[test]
    fn provider_eligibility_is_never_reported_as_a_hit_or_semantic_proof() {
        let CausalCacheDecisionV1::StrictReuse(admission) = validate_causal_cache_v1(
            verified_components(Some((
                CacheCoordinateV1::ProviderCache,
                CacheValidityV1::ProviderEligible,
            ))),
            &bound_invalidation(),
        )
        .unwrap() else {
            panic!("provider eligibility is orthogonal to semantic coordinates")
        };
        assert!(admission.record().provider_eligible);
        assert_eq!(admission.record().provider_reported_hit_tokens, None);
        assert!(CausalCacheComponentClaimV1::new(
            binding(),
            CacheCoordinateV1::Source,
            ArtifactOwnerV1::FsZero,
            CacheValidityV1::ProviderEligible,
            d(33),
            verifier_route(),
        )
        .is_err());
        assert!(CausalCacheComponentClaimV1::new(
            binding(),
            CacheCoordinateV1::ReasoningContinuation,
            ArtifactOwnerV1::TokenZero,
            CacheValidityV1::Exact,
            d(34),
            verifier_route(),
        )
        .is_err());
    }

    #[test]
    fn cache_authority_rejects_missing_components_and_unmatched_evidence() {
        let mut components = verified_components(None);
        components.pop();
        assert_eq!(
            validate_causal_cache_v1(components, &bound_invalidation())
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::IncompleteCoordinateSet
        );

        let mut unrelated_binding = binding();
        unrelated_binding.artifact_digest = d(99);
        let unrelated = BoundCausalCacheInvalidationV1::test_only(&unrelated_binding);
        assert_eq!(
            validate_causal_cache_v1(verified_components(None), &unrelated)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::InvalidationAuthorityMismatch
        );

        let claim = CausalCacheComponentClaimV1::new(
            binding(),
            CacheCoordinateV1::Source,
            ArtifactOwnerV1::FsZero,
            CacheValidityV1::Exact,
            d(35),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(b"not-the-claim".to_vec());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            VerifiedCausalCacheComponentV1::verify(claim, &evidence)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::EvidencePayloadMismatch
        );
    }

    fn metric_receipt(
        label: Q99LabelV1,
        numerator: u128,
        denominator: u128,
    ) -> VerifiedQ99MetricReceiptV1 {
        let claim = Q99MetricReceiptClaimV1::new(
            label,
            d(40),
            d(41),
            100,
            numerator,
            denominator,
            d(42),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedQ99MetricReceiptV1::verify(claim, &evidence).unwrap()
    }

    #[test]
    fn state_and_input_claims_have_labeled_denominators_and_exact_integer_thresholds() {
        let Q99ClaimDecisionV1::Attained(state) =
            generate_q99_metric_claim_v1(metric_receipt(Q99LabelV1::Q99State, 99, 100)).unwrap()
        else {
            panic!("99 of 100 exact state reuses must attain Q99-State")
        };
        assert_eq!(state.record().label, Q99LabelV1::Q99State);
        assert_eq!(
            state.record().threshold_relation,
            Q99ThresholdRelationV1::AtLeast99Of100
        );
        assert_eq!(state.record().observed_numerator, "99");
        assert_eq!(state.record().denominator, "100");
        state.record().validate().unwrap();
        let mut impossible = state.record().clone();
        impossible.observed_numerator = "101".into();
        impossible.attained = false;
        impossible.claim_digest = impossible.expected_digest().unwrap();
        assert_eq!(
            impossible.validate().unwrap_err().failure_code(),
            Q99FailureCodeV1::InvalidClaim
        );

        let Q99ClaimDecisionV1::NotAttained(input) =
            generate_q99_metric_claim_v1(metric_receipt(Q99LabelV1::Q99Input, 98, 100)).unwrap()
        else {
            panic!("98 of 100 cannot attain Q99-Input")
        };
        assert!(!input.attained);
        assert_eq!(input.label, Q99LabelV1::Q99Input);
        assert!(Q99MetricReceiptClaimV1::new(
            Q99LabelV1::Q99Total,
            d(1),
            d(2),
            1,
            1,
            1,
            d(3),
            verifier_route(),
        )
        .is_err());
    }

    fn work_receipt(
        total: u64,
        work_id: u8,
        boundary: u8,
        class: CausalWorkClassV1,
        unit: CausalCounterUnitV1,
    ) -> CausalWorkReceiptV1 {
        let identity = ParentCounterIdentityV1 {
            counter_id: "q99-complete-work".into(),
            unit,
            boundary_digest: d(boundary),
            adapter_digest: d(240),
            platform_profile_digest: d(241),
        };
        let CausalWorkOutcomeV1::Measured { receipt } = CausalWorkReceiptV1::build(
            d(242),
            ParentCounterObservationV1::Measured {
                window: ParentCounterWindowV1 {
                    identity,
                    start: 0,
                    end: total,
                },
            },
            vec![CausalWorkChargeV1 {
                work_unit_id: d(work_id),
                class,
                amount: total,
            }],
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap() else {
            panic!("measured fixture must produce a receipt")
        };
        receipt
    }

    fn verified_work(receipt: CausalWorkReceiptV1) -> VerifiedCausalWorkReceiptV1 {
        let bytes = canonical_causal_work_bytes(&receipt).unwrap();
        let (certificate, resolver) = certificate(bytes);
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedCausalWorkReceiptV1::verify(receipt, verifier_route(), &evidence).unwrap()
    }

    fn preparation(total: u64, work_id: u8) -> VerifiedQ99PreparationV1 {
        let work = verified_work(work_receipt(
            total,
            work_id,
            50,
            CausalWorkClassV1::Prewarm,
            CausalCounterUnitV1::Tokens,
        ));
        let claim =
            Q99PreparationClaimV1::new(d(40), d(41), work.receipt_digest(), verifier_route())
                .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedQ99PreparationV1::verify(claim, work, &evidence).unwrap()
    }

    fn task_pair(
        task: u8,
        baseline_total: u64,
        complete_total: u64,
        baseline_work_id: u8,
        complete_work_id: u8,
    ) -> VerifiedQ99TaskPairV1 {
        let baseline = verified_work(work_receipt(
            baseline_total,
            baseline_work_id,
            60 + task,
            CausalWorkClassV1::Baseline,
            CausalCounterUnitV1::Tokens,
        ));
        let complete = verified_work(work_receipt(
            complete_total,
            complete_work_id,
            90 + task,
            CausalWorkClassV1::Candidate,
            CausalCounterUnitV1::Tokens,
        ));
        let claim = Q99TaskPairClaimV1::new(
            d(40),
            d(41),
            d(task),
            baseline.receipt_digest(),
            complete.receipt_digest(),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedQ99TaskPairV1::verify(claim, baseline, complete, &evidence).unwrap()
    }

    #[test]
    fn total_claim_charges_preparation_and_complete_work_against_paired_raw_baseline() {
        let Q99ClaimDecisionV1::Attained(certificate) =
            generate_q99_total_claim_v1(preparation(1, 1), vec![task_pair(10, 1_000, 9, 2, 3)])
                .unwrap()
        else {
            panic!("one preparation plus nine residual must attain Q99-Total")
        };
        let record = certificate.record();
        assert_eq!(record.label, Q99LabelV1::Q99Total);
        assert_eq!(record.observed_numerator, "10");
        assert_eq!(record.denominator, "1000");
        assert_eq!(
            record.threshold_relation,
            Q99ThresholdRelationV1::AtMost1Of100
        );
        assert!(record.work_profile.is_some());
        let claim_keys = serde_json::to_value(record)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_claim_keys = [
            "schema_version",
            "label",
            "comparison_identity_digest",
            "workload_digest",
            "work_profile",
            "task_count",
            "observed_numerator",
            "denominator",
            "threshold_relation",
            "threshold_numerator",
            "threshold_denominator",
            "attained",
            "source_receipt_digests",
            "claim_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(claim_keys, expected_claim_keys);
        assert_eq!(serde_json::to_value(record).unwrap()["label"], "q99-total");
        record.validate().unwrap();
        let mut forged = record.clone();
        forged.observed_numerator = "9".into();
        assert_eq!(
            forged.validate().unwrap_err().failure_code(),
            Q99FailureCodeV1::InvalidClaim
        );
        let bytes = record.canonical_bytes().unwrap();
        assert_eq!(
            Q99ClaimRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *record
        );

        let Q99ClaimDecisionV1::NotAttained(record) =
            generate_q99_total_claim_v1(preparation(1, 4), vec![task_pair(11, 1_000, 10, 5, 6)])
                .unwrap()
        else {
            panic!("eleven complete units cannot attain Q99-Total")
        };
        assert!(!record.attained);
    }

    #[test]
    fn total_claim_rejects_double_counting_and_mixed_native_coordinates() {
        assert_eq!(
            generate_q99_total_claim_v1(preparation(1, 7), vec![task_pair(12, 1_000, 9, 8, 7)],)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::DuplicateWorkUnit
        );

        let baseline = verified_work(work_receipt(
            1_000,
            10,
            70,
            CausalWorkClassV1::Baseline,
            CausalCounterUnitV1::Tokens,
        ));
        let complete = verified_work(work_receipt(
            9,
            11,
            71,
            CausalWorkClassV1::Candidate,
            CausalCounterUnitV1::Bytes,
        ));
        let claim = Q99TaskPairClaimV1::new(
            d(40),
            d(41),
            d(13),
            baseline.receipt_digest(),
            complete.receipt_digest(),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            VerifiedQ99TaskPairV1::verify(claim, baseline, complete, &evidence)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::WorkProfileMismatch
        );
    }

    #[test]
    fn contract_and_external_schema_digests_are_stable() {
        // b06a2550ccb1b2f2d58c7adb0def2d804d300a7ec98daf12d87b5926de5f11a7
        assert_eq!(
            q99_contract_digest_v1(),
            DigestV1::from_bytes([
                0xb0, 0x6a, 0x25, 0x50, 0xcc, 0xb1, 0xb2, 0xf2, 0xd5, 0x8c, 0x7a, 0xdb, 0x0d, 0xef,
                0x2d, 0x80, 0x4d, 0x30, 0x0a, 0x7e, 0xc9, 0x8d, 0xaf, 0x12, 0xd8, 0x7b, 0x59, 0x26,
                0xde, 0x5f, 0x11, 0xa7,
            ])
        );
        // ae0fa6885e08f28bd68613eb6430be3c904874946336a0fae5da5f8cf2df8236
        assert_eq!(
            q99_contract_digest_v2(),
            DigestV1::from_bytes([
                0xae, 0x0f, 0xa6, 0x88, 0x5e, 0x08, 0xf2, 0x8b, 0xd6, 0x86, 0x13, 0xeb, 0x64, 0x30,
                0xbe, 0x3c, 0x90, 0x48, 0x74, 0x94, 0x63, 0x36, 0xa0, 0xfa, 0xe5, 0xda, 0x5f, 0x8c,
                0xf2, 0xdf, 0x82, 0x36,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../conformance/schemas/q99-causal-cache-component-v1.schema.json"
            )))
            .to_hex(),
            Q99_CACHE_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../conformance/schemas/q99-claim-v1.schema.json"
            )))
            .to_hex(),
            Q99_CLAIM_SCHEMA_SHA256_V1
        );
    }
}
