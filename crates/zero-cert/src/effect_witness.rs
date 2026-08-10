//! Verifier-owned Effect IR acceptance and structured witness carriers.
//!
//! `EffectAcceptedV1` has private fields and no deserializer or public
//! constructor. The only constructor consumes `VerifiedEvidence`, so raw JSON,
//! booleans, and prose cannot mint accepted authority.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{CwirVerifierClassV1, DigestV1, EffectProgramV1, canonical_json, sha256};

use crate::{CompletenessWitness, Query, VerifiedEvidence};

pub const EFFECT_WITNESS_CONTRACT_VERSION_V1: u16 = 1;
pub const EFFECT_WITNESS_DOMAIN_V1: &[u8] = b"zerostack.effect_witness.v1\0";
pub const EFFECT_ACCEPTED_DOMAIN_V1: &[u8] = b"zerostack.effect_verification.accepted.v1\0";
pub const EFFECT_EVIDENCE_REF_DOMAIN_V1: &[u8] = b"zerostack.effect_witness.evidence_ref.v1\0";
pub const EFFECT_WITNESS_MAX_CANONICAL_BYTES_V1: usize = 262_144;
pub const EFFECT_WITNESS_MAX_EVIDENCE_REFS_V1: usize = 512;
pub const EFFECT_WITNESS_MAX_EXPANSIONS_V1: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectWitnessFailureCodeV1 {
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
pub struct EffectWitnessErrorV1 {
    pub code: EffectWitnessFailureCodeV1,
    pub detail: String,
}

impl EffectWitnessErrorV1 {
    pub fn new(code: EffectWitnessFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> EffectWitnessFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for EffectWitnessErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for EffectWitnessErrorV1 {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectWitnessKindV1 {
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
pub enum EffectLocalizationClassV1 {
    Global,
    Predicate,
    Operation,
    Target,
    ByteRange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectLocalizationV1 {
    class: EffectLocalizationClassV1,
    operation_index: Option<u32>,
    target_digest: Option<DigestV1>,
    byte_start: Option<u64>,
    byte_len: Option<u64>,
}

impl EffectLocalizationV1 {
    pub const fn global() -> Self {
        Self {
            class: EffectLocalizationClassV1::Global,
            operation_index: None,
            target_digest: None,
            byte_start: None,
            byte_len: None,
        }
    }

    pub const fn predicate() -> Self {
        Self {
            class: EffectLocalizationClassV1::Predicate,
            operation_index: None,
            target_digest: None,
            byte_start: None,
            byte_len: None,
        }
    }

    pub const fn operation(operation_index: u32) -> Self {
        Self {
            class: EffectLocalizationClassV1::Operation,
            operation_index: Some(operation_index),
            target_digest: None,
            byte_start: None,
            byte_len: None,
        }
    }

    pub fn target(target_digest: DigestV1) -> Result<Self, EffectWitnessErrorV1> {
        require_digest("localization target", target_digest)?;
        Ok(Self {
            class: EffectLocalizationClassV1::Target,
            operation_index: None,
            target_digest: Some(target_digest),
            byte_start: None,
            byte_len: None,
        })
    }

    pub fn byte_range(
        target_digest: DigestV1,
        byte_start: u64,
        byte_len: u64,
    ) -> Result<Self, EffectWitnessErrorV1> {
        require_digest("localization target", target_digest)?;
        if byte_len == 0 || byte_start.checked_add(byte_len).is_none() {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::RangeOverflow,
                "byte localization is empty or overflows u64",
            ));
        }
        Ok(Self {
            class: EffectLocalizationClassV1::ByteRange,
            operation_index: None,
            target_digest: Some(target_digest),
            byte_start: Some(byte_start),
            byte_len: Some(byte_len),
        })
    }

    pub const fn class(&self) -> EffectLocalizationClassV1 {
        self.class
    }

    pub const fn operation_index(&self) -> Option<u32> {
        self.operation_index
    }

    pub const fn target_digest(&self) -> Option<DigestV1> {
        self.target_digest
    }

    pub const fn byte_range_value(&self) -> Option<(u64, u64)> {
        match (self.byte_start, self.byte_len) {
            (Some(start), Some(len)) => Some((start, len)),
            _ => None,
        }
    }

    fn validate(self) -> Result<(), EffectWitnessErrorV1> {
        let valid = match self.class {
            EffectLocalizationClassV1::Global | EffectLocalizationClassV1::Predicate => {
                self.operation_index.is_none()
                    && self.target_digest.is_none()
                    && self.byte_start.is_none()
                    && self.byte_len.is_none()
            }
            EffectLocalizationClassV1::Operation => {
                self.operation_index.is_some()
                    && self.target_digest.is_none()
                    && self.byte_start.is_none()
                    && self.byte_len.is_none()
            }
            EffectLocalizationClassV1::Target => {
                self.operation_index.is_none()
                    && self.target_digest.is_some()
                    && self.byte_start.is_none()
                    && self.byte_len.is_none()
            }
            EffectLocalizationClassV1::ByteRange => {
                self.operation_index.is_none()
                    && self.target_digest.is_some()
                    && self.byte_start.is_some()
                    && self.byte_len.is_some()
            }
        };
        if !valid {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::InvalidLocalization,
                "localization fields do not match the declared class",
            ));
        }
        if let Some(target) = self.target_digest {
            require_digest("localization target", target)?;
        }
        if let (Some(start), Some(len)) = (self.byte_start, self.byte_len)
            && (len == 0 || start.checked_add(len).is_none())
        {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::RangeOverflow,
                "byte localization is empty or overflows u64",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectWitnessV1 {
    contract_version: u16,
    cwir_semantic_digest: DigestV1,
    action_digest: DigestV1,
    obligation_id: DigestV1,
    kind: EffectWitnessKindV1,
    expected_predicate_digest: DigestV1,
    observed_evidence_digest: DigestV1,
    state_snapshot: DigestV1,
    localization: EffectLocalizationV1,
    exact_evidence_refs: Vec<DigestV1>,
    expansion_handles: Vec<DigestV1>,
    verifier_digest: DigestV1,
    witness_digest: DigestV1,
}

#[derive(Serialize)]
struct EffectWitnessBodyV1<'a> {
    contract_version: u16,
    cwir_semantic_digest: DigestV1,
    action_digest: DigestV1,
    obligation_id: DigestV1,
    kind: EffectWitnessKindV1,
    expected_predicate_digest: DigestV1,
    observed_evidence_digest: DigestV1,
    state_snapshot: DigestV1,
    localization: EffectLocalizationV1,
    exact_evidence_refs: &'a [DigestV1],
    expansion_handles: &'a [DigestV1],
    verifier_digest: DigestV1,
}

impl EffectWitnessV1 {
    pub const fn cwir_semantic_digest(&self) -> DigestV1 {
        self.cwir_semantic_digest
    }

    pub const fn action_digest(&self) -> DigestV1 {
        self.action_digest
    }

    pub const fn obligation_id(&self) -> DigestV1 {
        self.obligation_id
    }

    pub const fn kind(&self) -> EffectWitnessKindV1 {
        self.kind
    }

    pub const fn expected_predicate_digest(&self) -> DigestV1 {
        self.expected_predicate_digest
    }

    pub const fn observed_evidence_digest(&self) -> DigestV1 {
        self.observed_evidence_digest
    }

    pub const fn state_snapshot(&self) -> DigestV1 {
        self.state_snapshot
    }

    pub const fn localization(&self) -> EffectLocalizationV1 {
        self.localization
    }

    pub fn exact_evidence_refs(&self) -> &[DigestV1] {
        &self.exact_evidence_refs
    }

    pub fn expansion_handles(&self) -> &[DigestV1] {
        &self.expansion_handles
    }

    pub const fn verifier_digest(&self) -> DigestV1 {
        self.verifier_digest
    }

    pub const fn witness_digest(&self) -> DigestV1 {
        self.witness_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectWitnessErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > EFFECT_WITNESS_MAX_CANONICAL_BYTES_V1 {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::CanonicalPayloadTooLarge,
                format!("witness has {} canonical bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, EffectWitnessErrorV1> {
        if bytes.len() > EFFECT_WITNESS_MAX_CANONICAL_BYTES_V1 {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::CanonicalPayloadTooLarge,
                format!("witness has {} canonical bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::NonCanonicalEncoding,
                "witness bytes are not exact canonical JSON",
            ));
        }
        let witness: Self = serde_json::from_value(value).map_err(serialization_error)?;
        witness.validate()?;
        Ok(witness)
    }

    pub fn validate(&self) -> Result<(), EffectWitnessErrorV1> {
        if self.contract_version != EFFECT_WITNESS_CONTRACT_VERSION_V1 {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::UnsupportedVersion,
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
            EFFECT_WITNESS_MAX_EVIDENCE_REFS_V1,
            EffectWitnessFailureCodeV1::TooManyEvidenceRefs,
        )?;
        validate_sorted_set(
            &self.expansion_handles,
            "expansion_handles",
            EFFECT_WITNESS_MAX_EXPANSIONS_V1,
            EffectWitnessFailureCodeV1::TooManyExpansions,
        )?;
        let expected = digest_body(EFFECT_WITNESS_DOMAIN_V1, &self.body())?;
        if self.witness_digest != expected {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::WitnessDigestMismatch,
                format!(
                    "witness digest {} does not match canonical body {}",
                    self.witness_digest.to_hex(),
                    expected.to_hex()
                ),
            ));
        }
        Ok(())
    }

    fn body(&self) -> EffectWitnessBodyV1<'_> {
        EffectWitnessBodyV1 {
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
pub struct EffectAcceptedV1 {
    contract_version: u16,
    cwir_semantic_digest: DigestV1,
    action_digest: DigestV1,
    obligation_id: DigestV1,
    predicate_digest: DigestV1,
    state_snapshot: DigestV1,
    evidence_digest: DigestV1,
    verifier_digest: DigestV1,
    verifier_class: CwirVerifierClassV1,
    acceptance_digest: DigestV1,
}

#[derive(Serialize)]
struct EffectAcceptedBodyV1 {
    contract_version: u16,
    cwir_semantic_digest: DigestV1,
    action_digest: DigestV1,
    obligation_id: DigestV1,
    predicate_digest: DigestV1,
    state_snapshot: DigestV1,
    evidence_digest: DigestV1,
    verifier_digest: DigestV1,
    verifier_class: CwirVerifierClassV1,
}

impl EffectAcceptedV1 {
    pub const fn cwir_semantic_digest(&self) -> DigestV1 {
        self.cwir_semantic_digest
    }

    pub const fn action_digest(&self) -> DigestV1 {
        self.action_digest
    }

    pub const fn obligation_id(&self) -> DigestV1 {
        self.obligation_id
    }

    pub const fn predicate_digest(&self) -> DigestV1 {
        self.predicate_digest
    }

    pub const fn state_snapshot(&self) -> DigestV1 {
        self.state_snapshot
    }

    pub const fn evidence_digest(&self) -> DigestV1 {
        self.evidence_digest
    }

    pub const fn verifier_digest(&self) -> DigestV1 {
        self.verifier_digest
    }

    pub const fn verifier_class(&self) -> CwirVerifierClassV1 {
        self.verifier_class
    }

    pub const fn acceptance_digest(&self) -> DigestV1 {
        self.acceptance_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectWitnessErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > EFFECT_WITNESS_MAX_CANONICAL_BYTES_V1 {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::CanonicalPayloadTooLarge,
                format!("acceptance has {} canonical bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), EffectWitnessErrorV1> {
        if self.contract_version != EFFECT_WITNESS_CONTRACT_VERSION_V1 {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::UnsupportedVersion,
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
        let expected = digest_body(EFFECT_ACCEPTED_DOMAIN_V1, &self.body())?;
        if self.acceptance_digest != expected {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::AcceptanceDigestMismatch,
                "acceptance digest does not match canonical body",
            ));
        }
        Ok(())
    }

    fn body(&self) -> EffectAcceptedBodyV1 {
        EffectAcceptedBodyV1 {
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
pub enum EffectVerificationOutcomeV1 {
    Accepted(EffectAcceptedV1),
    Rejected(EffectWitnessV1),
    Incomplete(EffectWitnessV1),
}

#[allow(clippy::too_many_arguments)]
pub fn accept_effect_verification_v1(
    cwir_semantic_digest: DigestV1,
    program: &EffectProgramV1,
    obligation_id: DigestV1,
    predicate_digest: DigestV1,
    state_snapshot: DigestV1,
    verifier_digest: DigestV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectVerificationOutcomeV1, EffectWitnessErrorV1> {
    let verifier_class =
        validate_program_binding(program, state_snapshot, predicate_digest, verifier_digest)?;
    validate_evidence_snapshot(state_snapshot, evidence)?;
    let evidence_digest = verified_evidence_digest(evidence)?;
    let mut accepted = EffectAcceptedV1 {
        contract_version: EFFECT_WITNESS_CONTRACT_VERSION_V1,
        cwir_semantic_digest,
        action_digest: program.action_digest(),
        obligation_id,
        predicate_digest,
        state_snapshot,
        evidence_digest,
        verifier_digest,
        verifier_class,
        acceptance_digest: DigestV1::ZERO,
    };
    accepted.validate_body_digests()?;
    accepted.acceptance_digest = digest_body(EFFECT_ACCEPTED_DOMAIN_V1, &accepted.body())?;
    Ok(EffectVerificationOutcomeV1::Accepted(accepted))
}

#[allow(clippy::too_many_arguments)]
pub fn reject_effect_verification_v1(
    cwir_semantic_digest: DigestV1,
    program: &EffectProgramV1,
    obligation_id: DigestV1,
    kind: EffectWitnessKindV1,
    expected_predicate_digest: DigestV1,
    state_snapshot: DigestV1,
    localization: EffectLocalizationV1,
    mut expansion_handles: Vec<DigestV1>,
    verifier_digest: DigestV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectVerificationOutcomeV1, EffectWitnessErrorV1> {
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
    Ok(EffectVerificationOutcomeV1::Rejected(witness))
}

#[allow(clippy::too_many_arguments)]
pub fn incomplete_effect_verification_v1(
    cwir_semantic_digest: DigestV1,
    program: &EffectProgramV1,
    obligation_id: DigestV1,
    kind: EffectWitnessKindV1,
    expected_predicate_digest: DigestV1,
    state_snapshot: DigestV1,
    localization: EffectLocalizationV1,
    mut expansion_handles: Vec<DigestV1>,
    verifier_digest: DigestV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectVerificationOutcomeV1, EffectWitnessErrorV1> {
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
    Ok(EffectVerificationOutcomeV1::Incomplete(witness))
}

#[allow(clippy::too_many_arguments)]
fn build_witness(
    cwir_semantic_digest: DigestV1,
    program: &EffectProgramV1,
    obligation_id: DigestV1,
    kind: EffectWitnessKindV1,
    expected_predicate_digest: DigestV1,
    state_snapshot: DigestV1,
    localization: EffectLocalizationV1,
    expansion_handles: &mut [DigestV1],
    verifier_digest: DigestV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<EffectWitnessV1, EffectWitnessErrorV1> {
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
    if expansion_handles.len() > EFFECT_WITNESS_MAX_EXPANSIONS_V1 {
        return Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::TooManyExpansions,
            format!("witness has {} expansion handles", expansion_handles.len()),
        ));
    }
    let observed_evidence_digest = verified_evidence_digest(evidence)?;
    let exact_evidence_refs = exact_evidence_refs(evidence)?;
    let mut witness = EffectWitnessV1 {
        contract_version: EFFECT_WITNESS_CONTRACT_VERSION_V1,
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
        witness_digest: DigestV1::ZERO,
    };
    witness.validate_body_digests()?;
    witness.witness_digest = digest_body(EFFECT_WITNESS_DOMAIN_V1, &witness.body())?;
    witness.validate()?;
    Ok(witness)
}

fn validate_program_binding(
    program: &EffectProgramV1,
    state_snapshot: DigestV1,
    predicate_digest: DigestV1,
    verifier_digest: DigestV1,
) -> Result<CwirVerifierClassV1, EffectWitnessErrorV1> {
    program.validate().map_err(|error| {
        EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::InvalidEffectProgram,
            error.to_string(),
        )
    })?;
    if program.base_state() != state_snapshot {
        return Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::EffectStateMismatch,
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
        return Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::PredicateNotInPlan,
            "predicate is absent from the effect verification plan",
        ));
    }
    let Some(step) = program.verification().steps().iter().find(|step| {
        step.predicate_digest == predicate_digest && step.verifier_digest == verifier_digest
    }) else {
        return Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::VerificationBindingMismatch,
            "verifier and predicate are not paired in the effect verification plan",
        ));
    };
    Ok(step.verifier_class)
}

fn validate_localization_against(
    program: &EffectProgramV1,
    localization: EffectLocalizationV1,
) -> Result<(), EffectWitnessErrorV1> {
    match localization.class() {
        EffectLocalizationClassV1::Global | EffectLocalizationClassV1::Predicate => Ok(()),
        EffectLocalizationClassV1::Operation => {
            let Some(index) = localization.operation_index().map(|value| value as usize) else {
                return Err(EffectWitnessErrorV1::new(
                    EffectWitnessFailureCodeV1::InvalidLocalization,
                    "operation localization is missing its index",
                ));
            };
            if index < program.operations().len() {
                Ok(())
            } else {
                Err(EffectWitnessErrorV1::new(
                    EffectWitnessFailureCodeV1::InvalidLocalization,
                    format!("operation index {index} is outside the effect program"),
                ))
            }
        }
        EffectLocalizationClassV1::Target | EffectLocalizationClassV1::ByteRange => {
            let Some(target) = localization.target_digest() else {
                return Err(EffectWitnessErrorV1::new(
                    EffectWitnessFailureCodeV1::InvalidLocalization,
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
                Err(EffectWitnessErrorV1::new(
                    EffectWitnessFailureCodeV1::InvalidLocalization,
                    format!(
                        "localized target {} is absent from the effect program",
                        target.to_hex()
                    ),
                ))
            }
        }
    }
}

impl EffectAcceptedV1 {
    fn validate_body_digests(&self) -> Result<(), EffectWitnessErrorV1> {
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

impl EffectWitnessV1 {
    fn validate_body_digests(&self) -> Result<(), EffectWitnessErrorV1> {
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

pub fn effect_witness_contract_manifest_v1() -> Value {
    json!({
        "contract": "zerostack.effect_witness",
        "contract_version": EFFECT_WITNESS_CONTRACT_VERSION_V1,
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "domains": {
            "witness": "zerostack.effect_witness.v1\u{0}",
            "accepted": "zerostack.effect_verification.accepted.v1\u{0}",
            "evidence_ref": "zerostack.effect_witness.evidence_ref.v1\u{0}"
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
            "max_canonical_bytes": EFFECT_WITNESS_MAX_CANONICAL_BYTES_V1,
            "max_evidence_refs": EFFECT_WITNESS_MAX_EVIDENCE_REFS_V1,
            "max_expansions": EFFECT_WITNESS_MAX_EXPANSIONS_V1
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

pub fn effect_witness_contract_digest_v1() -> DigestV1 {
    DigestV1::from_bytes(sha256(
        canonical_json(&effect_witness_contract_manifest_v1()).as_bytes(),
    ))
}

fn verified_evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<DigestV1, EffectWitnessErrorV1> {
    evidence
        .certificate()
        .canonical_digest()
        .map(DigestV1::from_bytes)
        .map_err(serialization_error)
}

fn exact_evidence_refs(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Vec<DigestV1>, EffectWitnessErrorV1> {
    if evidence.spans().len() > EFFECT_WITNESS_MAX_EVIDENCE_REFS_V1 {
        return Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::TooManyEvidenceRefs,
            format!("verified evidence has {} span refs", evidence.spans().len()),
        ));
    }
    let mut refs = Vec::with_capacity(evidence.spans().len());
    for span in evidence.spans() {
        refs.push(digest_body(EFFECT_EVIDENCE_REF_DOMAIN_V1, span)?);
    }
    refs.sort();
    reject_duplicates(&refs, "exact evidence refs")?;
    Ok(refs)
}

fn validate_evidence_snapshot(
    expected: DigestV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), EffectWitnessErrorV1> {
    if let Some(observed) = evidence_snapshot(evidence) {
        require_snapshot(observed, expected)?;
    }
    Ok(())
}

fn evidence_snapshot(evidence: &VerifiedEvidence<'_, '_>) -> Option<DigestV1> {
    match evidence.query() {
        Query::ExactSearchDomain { snapshot_id, .. } | Query::Aggregate { snapshot_id, .. } => {
            Some(DigestV1::from_bytes(*snapshot_id))
        }
        _ => match &evidence.certificate().completeness {
            CompletenessWitness::ExactSearchDomain { snapshot_id, .. }
            | CompletenessWitness::Aggregate { snapshot_id, .. } => {
                Some(DigestV1::from_bytes(*snapshot_id))
            }
            _ => None,
        },
    }
}

fn require_snapshot(actual: DigestV1, expected: DigestV1) -> Result<(), EffectWitnessErrorV1> {
    if actual == expected {
        Ok(())
    } else {
        Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::StaleEvidence,
            format!(
                "evidence snapshot {} does not match effect state {}",
                actual.to_hex(),
                expected.to_hex()
            ),
        ))
    }
}

fn require_digest(label: &str, digest: DigestV1) -> Result<(), EffectWitnessErrorV1> {
    if digest == DigestV1::ZERO {
        Err(EffectWitnessErrorV1::new(
            EffectWitnessFailureCodeV1::ZeroDigest,
            format!("{label} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

fn validate_sorted_set(
    values: &[DigestV1],
    label: &str,
    max: usize,
    too_many: EffectWitnessFailureCodeV1,
) -> Result<(), EffectWitnessErrorV1> {
    if values.len() > max {
        return Err(EffectWitnessErrorV1::new(
            too_many,
            format!("{label} contains {} members", values.len()),
        ));
    }
    for digest in values {
        require_digest(label, *digest)?;
    }
    for pair in values.windows(2) {
        if pair[0] == pair[1] {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
        if pair[0] > pair[1] {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::NonCanonicalOrder,
                format!("{label} is not strictly sorted"),
            ));
        }
    }
    Ok(())
}

fn reject_duplicates<T: Eq>(values: &[T], label: &str) -> Result<(), EffectWitnessErrorV1> {
    for left in 0..values.len() {
        if values[left + 1..].contains(&values[left]) {
            return Err(EffectWitnessErrorV1::new(
                EffectWitnessFailureCodeV1::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
    }
    Ok(())
}

fn digest_body<T: Serialize>(domain: &[u8], value: &T) -> Result<DigestV1, EffectWitnessErrorV1> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let canonical = canonical_json(&value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Ok(DigestV1::from_bytes(sha256(&bytes)))
}

fn serialization_error(error: serde_json::Error) -> EffectWitnessErrorV1 {
    EffectWitnessErrorV1::new(
        EffectWitnessFailureCodeV1::SerializationFailure,
        error.to_string(),
    )
}
