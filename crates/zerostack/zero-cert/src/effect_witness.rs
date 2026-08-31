//! Verifier-owned Effect IR acceptance and structured witness carriers. `EffectAccepted`
//! has private fields and no deserializer or public constructor. The only constructor
//! consumes `VerifiedEvidence`, so raw JSON, booleans, and prose cannot mint accepted authority.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{CwirVerifierClass, EffectProgram, Sha256Digest, canonical_json, sha256};

use crate::{CompletenessWitness, Query, VerifiedEvidence};

pub const EFFECT_WITNESS_CONTRACT_VERSION: u16 = 1;
pub const EFFECT_WITNESS_DOMAIN: &[u8] = b"zerostack.effect_witness\0";
pub const EFFECT_ACCEPTED_DOMAIN: &[u8] = b"zerostack.effect_verification.accepted\0";
pub const EFFECT_EVIDENCE_REF_DOMAIN: &[u8] = b"zerostack.effect_witness.evidence_ref\0";
pub const EFFECT_WITNESS_MAX_CANONICAL_BYTES: usize = 262_144;
pub const EFFECT_WITNESS_MAX_EVIDENCE_REFS: usize = 512;
pub const EFFECT_WITNESS_MAX_EXPANSIONS: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectWitnessFailureCode {
    UnsupportedVersion,
    SerializationFailure,
    NonCanonicalEncoding,
    CanonicalPayloadTooLarge,
    ZeroDigest,
    DuplicateMember,
    NonCanonicalOrder,
    TooManyEvidenceRefs,
    TooManyExpansions,
    InvalidLocalization,
    RangeOverflow,
    StaleEvidence,
    EffectStateMismatch,
    InvalidEffectProgram,
    PredicateNotInPlan,
    VerificationBindingMismatch,
    WitnessDigestMismatch,
    AcceptanceDigestMismatch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectWitnessError {
    pub code: EffectWitnessFailureCode,
    pub detail: String,
}

impl EffectWitnessError {
    pub fn new(code: EffectWitnessFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> EffectWitnessFailureCode {
        self.code
    }
}

impl fmt::Display for EffectWitnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for EffectWitnessError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectWitnessKind {
    PredicateMismatch,
    ArtifactMismatch,
    StaleState,
    CapabilityMismatch,
    VerificationFailure,
    IncompleteCoverage,
    ExternalEffectUnsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectLocalizationClass {
    Global,
    Predicate,
    Operation,
    Target,
    ByteRange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectLocalization {
    class: EffectLocalizationClass,
    operation_index: Option<u32>,
    target_digest: Option<Sha256Digest>,
    byte_start: Option<u64>,
    byte_len: Option<u64>,
}

impl EffectLocalization {
    pub const fn global() -> Self {
        Self {
            class: EffectLocalizationClass::Global,
            operation_index: None,
            target_digest: None,
            byte_start: None,
            byte_len: None,
        }
    }

    pub const fn predicate() -> Self {
        Self {
            class: EffectLocalizationClass::Predicate,
            operation_index: None,
            target_digest: None,
            byte_start: None,
            byte_len: None,
        }
    }

    pub const fn operation(operation_index: u32) -> Self {
        Self {
            class: EffectLocalizationClass::Operation,
            operation_index: Some(operation_index),
            target_digest: None,
            byte_start: None,
            byte_len: None,
        }
    }

    pub fn target(target_digest: Sha256Digest) -> Result<Self, EffectWitnessError> {
        require_digest("localization target", target_digest)?;
        Ok(Self {
            class: EffectLocalizationClass::Target,
            operation_index: None,
            target_digest: Some(target_digest),
            byte_start: None,
            byte_len: None,
        })
    }

    pub fn byte_range(
        target_digest: Sha256Digest,
        byte_start: u64,
        byte_len: u64,
    ) -> Result<Self, EffectWitnessError> {
        require_digest("localization target", target_digest)?;
        if byte_len == 0 || byte_start.checked_add(byte_len).is_none() {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::RangeOverflow,
                "byte localization is empty or overflows u64",
            ));
        }
        Ok(Self {
            class: EffectLocalizationClass::ByteRange,
            operation_index: None,
            target_digest: Some(target_digest),
            byte_start: Some(byte_start),
            byte_len: Some(byte_len),
        })
    }

    pub const fn class(&self) -> EffectLocalizationClass {
        self.class
    }

    pub const fn operation_index(&self) -> Option<u32> {
        self.operation_index
    }

    pub const fn target_digest(&self) -> Option<Sha256Digest> {
        self.target_digest
    }

    pub const fn byte_range_value(&self) -> Option<(u64, u64)> {
        match (self.byte_start, self.byte_len) {
            (Some(start), Some(len)) => Some((start, len)),
            _ => None,
        }
    }

    fn validate(self) -> Result<(), EffectWitnessError> {
        let valid = match self.class {
            EffectLocalizationClass::Global | EffectLocalizationClass::Predicate => {
                self.operation_index.is_none()
                    && self.target_digest.is_none()
                    && self.byte_start.is_none()
                    && self.byte_len.is_none()
            }
            EffectLocalizationClass::Operation => {
                self.operation_index.is_some()
                    && self.target_digest.is_none()
                    && self.byte_start.is_none()
                    && self.byte_len.is_none()
            }
            EffectLocalizationClass::Target => {
                self.operation_index.is_none()
                    && self.target_digest.is_some()
                    && self.byte_start.is_none()
                    && self.byte_len.is_none()
            }
            EffectLocalizationClass::ByteRange => {
                self.operation_index.is_none()
                    && self.target_digest.is_some()
                    && self.byte_start.is_some()
                    && self.byte_len.is_some()
            }
        };
        if !valid {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::InvalidLocalization,
                "localization fields do not match the declared class",
            ));
        }
        if let Some(target) = self.target_digest {
            require_digest("localization target", target)?;
        }
        if let (Some(start), Some(len)) = (self.byte_start, self.byte_len)
            && (len == 0 || start.checked_add(len).is_none())
        {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::RangeOverflow,
                "byte localization is empty or overflows u64",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectWitness {
    contract_version: u16,
    cwir_semantic_digest: Sha256Digest,
    action_digest: Sha256Digest,
    obligation_id: Sha256Digest,
    kind: EffectWitnessKind,
    expected_predicate_digest: Sha256Digest,
    observed_evidence_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    localization: EffectLocalization,
    exact_evidence_refs: Vec<Sha256Digest>,
    expansion_handles: Vec<Sha256Digest>,
    verifier_digest: Sha256Digest,
    witness_digest: Sha256Digest,
}

#[derive(Serialize)]
struct EffectWitnessBody<'a> {
    contract_version: u16,
    cwir_semantic_digest: Sha256Digest,
    action_digest: Sha256Digest,
    obligation_id: Sha256Digest,
    kind: EffectWitnessKind,
    expected_predicate_digest: Sha256Digest,
    observed_evidence_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    localization: EffectLocalization,
    exact_evidence_refs: &'a [Sha256Digest],
    expansion_handles: &'a [Sha256Digest],
    verifier_digest: Sha256Digest,
}

impl EffectWitness {
    pub const fn cwir_semantic_digest(&self) -> Sha256Digest {
        self.cwir_semantic_digest
    }

    pub const fn action_digest(&self) -> Sha256Digest {
        self.action_digest
    }

    pub const fn obligation_id(&self) -> Sha256Digest {
        self.obligation_id
    }

    pub const fn kind(&self) -> EffectWitnessKind {
        self.kind
    }

    pub const fn expected_predicate_digest(&self) -> Sha256Digest {
        self.expected_predicate_digest
    }

    pub const fn observed_evidence_digest(&self) -> Sha256Digest {
        self.observed_evidence_digest
    }

    pub const fn state_snapshot(&self) -> Sha256Digest {
        self.state_snapshot
    }

    pub const fn localization(&self) -> EffectLocalization {
        self.localization
    }

    pub fn exact_evidence_refs(&self) -> &[Sha256Digest] {
        &self.exact_evidence_refs
    }

    pub fn expansion_handles(&self) -> &[Sha256Digest] {
        &self.expansion_handles
    }

    pub const fn verifier_digest(&self) -> Sha256Digest {
        self.verifier_digest
    }

    pub const fn witness_digest(&self) -> Sha256Digest {
        self.witness_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectWitnessError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > EFFECT_WITNESS_MAX_CANONICAL_BYTES {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::CanonicalPayloadTooLarge,
                format!("witness has {} canonical bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, EffectWitnessError> {
        if bytes.len() > EFFECT_WITNESS_MAX_CANONICAL_BYTES {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::CanonicalPayloadTooLarge,
                format!("witness has {} canonical bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::NonCanonicalEncoding,
                "witness bytes are not exact canonical JSON",
            ));
        }
        let witness: Self = serde_json::from_value(value).map_err(serialization_error)?;
        witness.validate()?;
        Ok(witness)
    }

    pub fn validate(&self) -> Result<(), EffectWitnessError> {
        if self.contract_version != EFFECT_WITNESS_CONTRACT_VERSION {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::UnsupportedVersion,
                format!(
                    "unsupported effect witness version {}",
                    self.contract_version
                ),
            ));
        }
        for (label, digest) in [
            ("cwir_semantic_digest", self.cwir_semantic_digest),
            ("action_digest", self.action_digest),
            ("obligation_id", self.obligation_id),
            ("expected_predicate_digest", self.expected_predicate_digest),
            ("observed_evidence_digest", self.observed_evidence_digest),
            ("state_snapshot", self.state_snapshot),
            ("verifier_digest", self.verifier_digest),
        ] {
            require_digest(label, digest)?;
        }
        self.localization.validate()?;
        validate_sorted_set(
            &self.exact_evidence_refs,
            "exact_evidence_refs",
            EFFECT_WITNESS_MAX_EVIDENCE_REFS,
            EffectWitnessFailureCode::TooManyEvidenceRefs,
        )?;
        validate_sorted_set(
            &self.expansion_handles,
            "expansion_handles",
            EFFECT_WITNESS_MAX_EXPANSIONS,
            EffectWitnessFailureCode::TooManyExpansions,
        )?;
        let expected = digest_body(EFFECT_WITNESS_DOMAIN, &self.body())?;
        if self.witness_digest != expected {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::WitnessDigestMismatch,
                format!(
                    "witness digest {} does not match canonical body {}",
                    self.witness_digest.to_hex(),
                    expected.to_hex()
                ),
            ));
        }
        Ok(())
    }

    fn body(&self) -> EffectWitnessBody<'_> {
        EffectWitnessBody {
            contract_version: self.contract_version,
            cwir_semantic_digest: self.cwir_semantic_digest,
            action_digest: self.action_digest,
            obligation_id: self.obligation_id,
            kind: self.kind,
            expected_predicate_digest: self.expected_predicate_digest,
            observed_evidence_digest: self.observed_evidence_digest,
            state_snapshot: self.state_snapshot,
            localization: self.localization,
            exact_evidence_refs: &self.exact_evidence_refs,
            expansion_handles: &self.expansion_handles,
            verifier_digest: self.verifier_digest,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectAccepted {
    contract_version: u16,
    cwir_semantic_digest: Sha256Digest,
    action_digest: Sha256Digest,
    obligation_id: Sha256Digest,
    predicate_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_digest: Sha256Digest,
    verifier_class: CwirVerifierClass,
    acceptance_digest: Sha256Digest,
}

#[derive(Serialize)]
struct EffectAcceptedBody {
    contract_version: u16,
    cwir_semantic_digest: Sha256Digest,
    action_digest: Sha256Digest,
    obligation_id: Sha256Digest,
    predicate_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_digest: Sha256Digest,
    verifier_class: CwirVerifierClass,
}

impl EffectAccepted {
    pub const fn cwir_semantic_digest(&self) -> Sha256Digest {
        self.cwir_semantic_digest
    }

    pub const fn action_digest(&self) -> Sha256Digest {
        self.action_digest
    }

    pub const fn obligation_id(&self) -> Sha256Digest {
        self.obligation_id
    }

    pub const fn predicate_digest(&self) -> Sha256Digest {
        self.predicate_digest
    }

    pub const fn state_snapshot(&self) -> Sha256Digest {
        self.state_snapshot
    }

    pub const fn evidence_digest(&self) -> Sha256Digest {
        self.evidence_digest
    }

    pub const fn verifier_digest(&self) -> Sha256Digest {
        self.verifier_digest
    }

    pub const fn verifier_class(&self) -> CwirVerifierClass {
        self.verifier_class
    }

    pub const fn acceptance_digest(&self) -> Sha256Digest {
        self.acceptance_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectWitnessError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > EFFECT_WITNESS_MAX_CANONICAL_BYTES {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::CanonicalPayloadTooLarge,
                format!("acceptance has {} canonical bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), EffectWitnessError> {
        if self.contract_version != EFFECT_WITNESS_CONTRACT_VERSION {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::UnsupportedVersion,
                format!(
                    "unsupported effect acceptance version {}",
                    self.contract_version
                ),
            ));
        }
        for (label, digest) in [
            ("cwir_semantic_digest", self.cwir_semantic_digest),
            ("action_digest", self.action_digest),
            ("obligation_id", self.obligation_id),
            ("predicate_digest", self.predicate_digest),
            ("state_snapshot", self.state_snapshot),
            ("evidence_digest", self.evidence_digest),
            ("verifier_digest", self.verifier_digest),
        ] {
            require_digest(label, digest)?;
        }
        let expected = digest_body(EFFECT_ACCEPTED_DOMAIN, &self.body())?;
        if self.acceptance_digest != expected {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::AcceptanceDigestMismatch,
                "acceptance digest does not match canonical body",
            ));
        }
        Ok(())
    }

    fn body(&self) -> EffectAcceptedBody {
        EffectAcceptedBody {
            contract_version: self.contract_version,
            cwir_semantic_digest: self.cwir_semantic_digest,
            action_digest: self.action_digest,
            obligation_id: self.obligation_id,
            predicate_digest: self.predicate_digest,
            state_snapshot: self.state_snapshot,
            evidence_digest: self.evidence_digest,
            verifier_digest: self.verifier_digest,
            verifier_class: self.verifier_class,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectVerificationOutcome {
    Accepted(EffectAccepted),
    Rejected(EffectWitness),
    Incomplete(EffectWitness),
}

#[allow(clippy::too_many_arguments)]
pub fn accept_effect_verification(
    cwir_semantic_digest: Sha256Digest,
    program: &EffectProgram,
    obligation_id: Sha256Digest,
    predicate_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    verifier_digest: Sha256Digest,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectVerificationOutcome, EffectWitnessError> {
    let verifier_class =
        validate_program_binding(program, state_snapshot, predicate_digest, verifier_digest)?;
    validate_evidence_snapshot(state_snapshot, evidence)?;
    let evidence_digest = verified_evidence_digest(evidence)?;
    let mut accepted = EffectAccepted {
        contract_version: EFFECT_WITNESS_CONTRACT_VERSION,
        cwir_semantic_digest,
        action_digest: program.action_digest(),
        obligation_id,
        predicate_digest,
        state_snapshot,
        evidence_digest,
        verifier_digest,
        verifier_class,
        acceptance_digest: Sha256Digest::ZERO,
    };
    accepted.validate_body_digests()?;
    accepted.acceptance_digest = digest_body(EFFECT_ACCEPTED_DOMAIN, &accepted.body())?;
    Ok(EffectVerificationOutcome::Accepted(accepted))
}

#[allow(clippy::too_many_arguments)]
pub fn reject_effect_verification(
    cwir_semantic_digest: Sha256Digest,
    program: &EffectProgram,
    obligation_id: Sha256Digest,
    kind: EffectWitnessKind,
    expected_predicate_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    localization: EffectLocalization,
    mut expansion_handles: Vec<Sha256Digest>,
    verifier_digest: Sha256Digest,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectVerificationOutcome, EffectWitnessError> {
    let witness = build_witness(
        cwir_semantic_digest,
        program,
        obligation_id,
        kind,
        expected_predicate_digest,
        state_snapshot,
        localization,
        &mut expansion_handles,
        verifier_digest,
        evidence,
    )?;
    Ok(EffectVerificationOutcome::Rejected(witness))
}

#[allow(clippy::too_many_arguments)]
pub fn incomplete_effect_verification(
    cwir_semantic_digest: Sha256Digest,
    program: &EffectProgram,
    obligation_id: Sha256Digest,
    kind: EffectWitnessKind,
    expected_predicate_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    localization: EffectLocalization,
    mut expansion_handles: Vec<Sha256Digest>,
    verifier_digest: Sha256Digest,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectVerificationOutcome, EffectWitnessError> {
    let witness = build_witness(
        cwir_semantic_digest,
        program,
        obligation_id,
        kind,
        expected_predicate_digest,
        state_snapshot,
        localization,
        &mut expansion_handles,
        verifier_digest,
        evidence,
    )?;
    Ok(EffectVerificationOutcome::Incomplete(witness))
}

#[allow(clippy::too_many_arguments)]
fn build_witness(
    cwir_semantic_digest: Sha256Digest,
    program: &EffectProgram,
    obligation_id: Sha256Digest,
    kind: EffectWitnessKind,
    expected_predicate_digest: Sha256Digest,
    state_snapshot: Sha256Digest,
    localization: EffectLocalization,
    expansion_handles: &mut [Sha256Digest],
    verifier_digest: Sha256Digest,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectWitness, EffectWitnessError> {
    validate_program_binding(
        program,
        state_snapshot,
        expected_predicate_digest,
        verifier_digest,
    )?;
    validate_evidence_snapshot(state_snapshot, evidence)?;
    localization.validate()?;
    validate_localization_against(program, localization)?;
    expansion_handles.sort();
    reject_duplicates(expansion_handles, "expansion handles")?;
    if expansion_handles.len() > EFFECT_WITNESS_MAX_EXPANSIONS {
        return Err(EffectWitnessError::new(
            EffectWitnessFailureCode::TooManyExpansions,
            format!("witness has {} expansion handles", expansion_handles.len()),
        ));
    }
    let observed_evidence_digest = verified_evidence_digest(evidence)?;
    let exact_evidence_refs = exact_evidence_refs(evidence)?;
    let mut witness = EffectWitness {
        contract_version: EFFECT_WITNESS_CONTRACT_VERSION,
        cwir_semantic_digest,
        action_digest: program.action_digest(),
        obligation_id,
        kind,
        expected_predicate_digest,
        observed_evidence_digest,
        state_snapshot,
        localization,
        exact_evidence_refs,
        expansion_handles: expansion_handles.to_vec(),
        verifier_digest,
        witness_digest: Sha256Digest::ZERO,
    };
    witness.validate_body_digests()?;
    witness.witness_digest = digest_body(EFFECT_WITNESS_DOMAIN, &witness.body())?;
    witness.validate()?;
    Ok(witness)
}

fn validate_program_binding(
    program: &EffectProgram,
    state_snapshot: Sha256Digest,
    predicate_digest: Sha256Digest,
    verifier_digest: Sha256Digest,
) -> Result<CwirVerifierClass, EffectWitnessError> {
    program.validate().map_err(|error| {
        EffectWitnessError::new(
            EffectWitnessFailureCode::InvalidEffectProgram,
            error.to_string(),
        )
    })?;
    if program.base_state() != state_snapshot {
        return Err(EffectWitnessError::new(
            EffectWitnessFailureCode::EffectStateMismatch,
            format!(
                "effect base {} does not match verification state {}",
                program.base_state().to_hex(),
                state_snapshot.to_hex()
            ),
        ));
    }
    require_digest("predicate_digest", predicate_digest)?;
    require_digest("verifier_digest", verifier_digest)?;
    if !program
        .verification()
        .steps()
        .iter()
        .any(|step| step.predicate_digest == predicate_digest)
    {
        return Err(EffectWitnessError::new(
            EffectWitnessFailureCode::PredicateNotInPlan,
            "predicate is absent from the effect verification plan",
        ));
    }
    let Some(step) = program.verification().steps().iter().find(|step| {
        step.predicate_digest == predicate_digest && step.verifier_digest == verifier_digest
    }) else {
        return Err(EffectWitnessError::new(
            EffectWitnessFailureCode::VerificationBindingMismatch,
            "verifier and predicate are not paired in the effect verification plan",
        ));
    };
    Ok(step.verifier_class)
}

fn validate_localization_against(
    program: &EffectProgram,
    localization: EffectLocalization,
) -> Result<(), EffectWitnessError> {
    match localization.class() {
        EffectLocalizationClass::Global | EffectLocalizationClass::Predicate => Ok(()),
        EffectLocalizationClass::Operation => {
            let Some(index) = localization.operation_index().map(|value| value as usize) else {
                return Err(EffectWitnessError::new(
                    EffectWitnessFailureCode::InvalidLocalization,
                    "operation localization is missing its index",
                ));
            };
            if index < program.operations().len() {
                Ok(())
            } else {
                Err(EffectWitnessError::new(
                    EffectWitnessFailureCode::InvalidLocalization,
                    format!("operation index {index} is outside the effect program"),
                ))
            }
        }
        EffectLocalizationClass::Target | EffectLocalizationClass::ByteRange => {
            let Some(target) = localization.target_digest() else {
                return Err(EffectWitnessError::new(
                    EffectWitnessFailureCode::InvalidLocalization,
                    "target localization is missing its target digest",
                ));
            };
            if program
                .targets()
                .iter()
                .any(|candidate| candidate.target_digest == target)
            {
                Ok(())
            } else {
                Err(EffectWitnessError::new(
                    EffectWitnessFailureCode::InvalidLocalization,
                    format!(
                        "localized target {} is absent from the effect program",
                        target.to_hex()
                    ),
                ))
            }
        }
    }
}

impl EffectAccepted {
    fn validate_body_digests(&self) -> Result<(), EffectWitnessError> {
        for (label, digest) in [
            ("cwir_semantic_digest", self.cwir_semantic_digest),
            ("action_digest", self.action_digest),
            ("obligation_id", self.obligation_id),
            ("predicate_digest", self.predicate_digest),
            ("state_snapshot", self.state_snapshot),
            ("evidence_digest", self.evidence_digest),
            ("verifier_digest", self.verifier_digest),
        ] {
            require_digest(label, digest)?;
        }
        Ok(())
    }
}

impl EffectWitness {
    fn validate_body_digests(&self) -> Result<(), EffectWitnessError> {
        for (label, digest) in [
            ("cwir_semantic_digest", self.cwir_semantic_digest),
            ("action_digest", self.action_digest),
            ("obligation_id", self.obligation_id),
            ("expected_predicate_digest", self.expected_predicate_digest),
            ("observed_evidence_digest", self.observed_evidence_digest),
            ("state_snapshot", self.state_snapshot),
            ("verifier_digest", self.verifier_digest),
        ] {
            require_digest(label, digest)?;
        }
        Ok(())
    }
}

pub fn effect_witness_contract_manifest() -> Value {
    json!({
        "contract": "zerostack.effect_witness",
        "contract_version": EFFECT_WITNESS_CONTRACT_VERSION,
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "domains": {
            "witness": "zerostack.effect_witness\u{0}",
            "accepted": "zerostack.effect_verification.accepted\u{0}",
            "evidence_ref": "zerostack.effect_witness.evidence_ref\u{0}"
        },
        "outcomes": ["accepted", "rejected", "incomplete"],
        "witness_fields": [
            "contract_version", "cwir_semantic_digest", "action_digest", "obligation_id",
            "kind", "expected_predicate_digest", "observed_evidence_digest", "state_snapshot",
            "localization", "exact_evidence_refs", "expansion_handles", "verifier_digest",
            "witness_digest"
        ],
        "accepted_fields": [
            "contract_version", "cwir_semantic_digest", "action_digest", "obligation_id",
            "predicate_digest", "state_snapshot", "evidence_digest", "verifier_digest",
            "verifier_class", "acceptance_digest"
        ],
        "witness_kinds": [
            "predicate_mismatch", "artifact_mismatch", "stale_state", "capability_mismatch",
            "verification_failure", "incomplete_coverage", "external_effect_unsupported"
        ],
        "localization_classes": ["global", "predicate", "operation", "target", "byte_range"],
        "verifier_classes": ["exact_checker", "sound_restricted", "empirical_incomplete"],
        "failure_codes": [
            "unsupported_version", "serialization_failure", "non_canonical_encoding",
            "canonical_payload_too_large", "zero_digest", "duplicate_member",
            "non_canonical_order", "too_many_evidence_refs", "too_many_expansions",
            "invalid_localization", "range_overflow", "stale_evidence",
            "effect_state_mismatch", "invalid_effect_program", "predicate_not_in_plan",
            "verification_binding_mismatch", "witness_digest_mismatch",
            "acceptance_digest_mismatch"
        ],
        "bounds": {
            "max_canonical_bytes": EFFECT_WITNESS_MAX_CANONICAL_BYTES,
            "max_evidence_refs": EFFECT_WITNESS_MAX_EVIDENCE_REFS,
            "max_expansions": EFFECT_WITNESS_MAX_EXPANSIONS
        },
        "invariants": [
            "accepted_outcomes_require_verified_evidence",
            "witnesses_bind_cwir_action_obligation_state_verifier_and_exact_evidence",
            "evidence_snapshot_scope_must_match_effect_state_when_present",
            "rejected_and_incomplete_are_distinct",
            "volatile_timings_are_excluded"
        ]
    })
}

pub fn effect_witness_contract_digest() -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(
        canonical_json(&effect_witness_contract_manifest()).as_bytes(),
    ))
}

fn verified_evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Sha256Digest, EffectWitnessError> {
    evidence
        .certificate()
        .canonical_digest()
        .map(Sha256Digest::from_bytes)
        .map_err(serialization_error)
}

fn exact_evidence_refs(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Vec<Sha256Digest>, EffectWitnessError> {
    if evidence.spans().len() > EFFECT_WITNESS_MAX_EVIDENCE_REFS {
        return Err(EffectWitnessError::new(
            EffectWitnessFailureCode::TooManyEvidenceRefs,
            format!("verified evidence has {} span refs", evidence.spans().len()),
        ));
    }
    let mut refs = Vec::with_capacity(evidence.spans().len());
    for span in evidence.spans() {
        refs.push(digest_body(EFFECT_EVIDENCE_REF_DOMAIN, span)?);
    }
    refs.sort();
    reject_duplicates(&refs, "exact evidence refs")?;
    Ok(refs)
}

fn validate_evidence_snapshot(
    expected: Sha256Digest,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), EffectWitnessError> {
    if let Some(observed) = evidence_snapshot(evidence) {
        require_snapshot(observed, expected)?;
    }
    Ok(())
}

fn evidence_snapshot(evidence: &VerifiedEvidence<'_, '_>) -> Option<Sha256Digest> {
    match evidence.query() {
        Query::ExactSearchDomain { snapshot_id, .. } | Query::Aggregate { snapshot_id, .. } => {
            Some(Sha256Digest::from_bytes(*snapshot_id))
        }
        _ => match &evidence.certificate().completeness {
            CompletenessWitness::ExactSearchDomain { snapshot_id, .. }
            | CompletenessWitness::Aggregate { snapshot_id, .. } => {
                Some(Sha256Digest::from_bytes(*snapshot_id))
            }
            _ => None,
        },
    }
}

fn require_snapshot(
    actual: Sha256Digest,
    expected: Sha256Digest,
) -> Result<(), EffectWitnessError> {
    if actual == expected {
        Ok(())
    } else {
        Err(EffectWitnessError::new(
            EffectWitnessFailureCode::StaleEvidence,
            format!(
                "evidence snapshot {} does not match effect state {}",
                actual.to_hex(),
                expected.to_hex()
            ),
        ))
    }
}

fn require_digest(label: &str, digest: Sha256Digest) -> Result<(), EffectWitnessError> {
    if digest == Sha256Digest::ZERO {
        Err(EffectWitnessError::new(
            EffectWitnessFailureCode::ZeroDigest,
            format!("{label} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

fn validate_sorted_set(
    values: &[Sha256Digest],
    label: &str,
    max: usize,
    too_many: EffectWitnessFailureCode,
) -> Result<(), EffectWitnessError> {
    if values.len() > max {
        return Err(EffectWitnessError::new(
            too_many,
            format!("{label} contains {} members", values.len()),
        ));
    }
    for digest in values {
        require_digest(label, *digest)?;
    }
    for pair in values.windows(2) {
        if pair[0] == pair[1] {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
        if pair[0] > pair[1] {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::NonCanonicalOrder,
                format!("{label} is not strictly sorted"),
            ));
        }
    }
    Ok(())
}

fn reject_duplicates<T: Eq>(values: &[T], label: &str) -> Result<(), EffectWitnessError> {
    for left in 0..values.len() {
        if values[left + 1..].contains(&values[left]) {
            return Err(EffectWitnessError::new(
                EffectWitnessFailureCode::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
    }
    Ok(())
}

fn digest_body<T: Serialize>(domain: &[u8], value: &T) -> Result<Sha256Digest, EffectWitnessError> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let canonical = canonical_json(&value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Ok(Sha256Digest::from_bytes(sha256(&bytes)))
}

fn serialization_error(error: serde_json::Error) -> EffectWitnessError {
    EffectWitnessError::new(
        EffectWitnessFailureCode::SerializationFailure,
        error.to_string(),
    )
}
