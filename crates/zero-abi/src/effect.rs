//! Typed Effect IR v1 for model-selected, verifier-gated actions.
//!
//! An effect program describes an action. It never grants permission to execute
//! it. Callers must still bind the program to the current state and capability
//! generation, isolate effects, obtain verifier evidence, and pass policy gates.
//! Raw fallback is a first-class operation and cannot be mixed with typed work.
//!
//! The production v1 identity uses canonical sorted-key JSON. The V2 prototype's
//! proposed binary codec remains a separate future contract and is not implied by
//! this module.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ArtifactOwnerV1, CwirVerifierClassV1, DigestV1, EffectClass, canonical_json, sha256};

pub const EFFECT_IR_CONTRACT_VERSION_V1: u16 = 1;
pub const EFFECT_IR_ACTION_DOMAIN_V1: &[u8] = b"zerostack.effect_ir.action.v1\0";
pub const EFFECT_IR_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const EFFECT_IR_MAX_OPERATIONS_V1: usize = 256;
pub const EFFECT_IR_MAX_TARGETS_V1: usize = 512;
pub const EFFECT_IR_MAX_PRECONDITIONS_V1: usize = 512;
pub const EFFECT_IR_MAX_EXCEPTIONS_V1: usize = 512;
pub const EFFECT_IR_MAX_VERIFICATION_STEPS_V1: usize = 128;
pub const EFFECT_IR_MAX_CAPABILITIES_V1: usize = 512;
pub const EFFECT_IR_MAX_INTENTS_V1: usize = 256;
pub const EFFECT_IR_MAX_STRING_BYTES_V1: usize = 256;
pub const EFFECT_IR_MAX_LITERAL_BYTES_V1: usize = 65_536;
pub const EFFECT_IR_MAX_REFS_PER_OPERATION_V1: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectIrFailureCodeV1 {
    UnsupportedVersion,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    SerializationFailure,
    InvalidIdentity,
    ZeroDigest,
    ZeroGeneration,
    DuplicateMember,
    NonCanonicalOrder,
    TooManyOperations,
    TooManyTargets,
    TooManyPreconditions,
    TooManyExceptions,
    TooManyVerificationSteps,
    TooManyCapabilities,
    TooManyIntents,
    LiteralTooLarge,
    LiteralDigestMismatch,
    ActionDigestMismatch,
    StaleBaseState,
    UnlistedIntent,
    UnlistedCapability,
    CapabilityGenerationMismatch,
    CapabilityContractMismatch,
    CapabilityEffectClassExceeded,
    MissingTarget,
    MissingException,
    InvalidOperation,
    RawFallbackMixed,
    RollbackTooWeak,
    VerificationRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectIrErrorV1 {
    pub code: EffectIrFailureCodeV1,
    pub detail: String,
}

impl EffectIrErrorV1 {
    pub fn new(code: EffectIrFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> EffectIrFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for EffectIrErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for EffectIrErrorV1 {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectRollbackV1 {
    ReadOnly,
    SingleAtomic,
    Journaled,
    WorkspaceClone,
    ExternalTransaction,
    RawFallback,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectTargetV1 {
    pub owner: ArtifactOwnerV1,
    pub target_digest: DigestV1,
    pub required_snapshot: DigestV1,
}

impl EffectTargetV1 {
    fn validate(self, base_state: DigestV1) -> Result<(), EffectIrErrorV1> {
        require_digest("target_digest", self.target_digest)?;
        require_digest("target.required_snapshot", self.required_snapshot)?;
        require_snapshot("target", self.required_snapshot, base_state)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectPredicateV1 {
    pub predicate_digest: DigestV1,
    pub scope_digest: DigestV1,
    pub required_snapshot: DigestV1,
}

impl EffectPredicateV1 {
    fn validate(self, base_state: DigestV1) -> Result<(), EffectIrErrorV1> {
        require_digest("predicate_digest", self.predicate_digest)?;
        require_digest("predicate.scope_digest", self.scope_digest)?;
        require_digest("predicate.required_snapshot", self.required_snapshot)?;
        require_snapshot("predicate", self.required_snapshot, base_state)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectExceptionV1 {
    pub target_digest: DigestV1,
    pub exception_digest: DigestV1,
}

impl EffectExceptionV1 {
    fn validate(self) -> Result<(), EffectIrErrorV1> {
        require_digest("exception.target_digest", self.target_digest)?;
        require_digest("exception.exception_digest", self.exception_digest)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectVerificationStepV1 {
    pub verifier_digest: DigestV1,
    pub predicate_digest: DigestV1,
    pub environment_digest: DigestV1,
    pub required_snapshot: DigestV1,
    pub verifier_class: CwirVerifierClassV1,
}

impl EffectVerificationStepV1 {
    fn validate(self, base_state: DigestV1) -> Result<(), EffectIrErrorV1> {
        require_digest("verification.verifier_digest", self.verifier_digest)?;
        require_digest("verification.predicate_digest", self.predicate_digest)?;
        require_digest("verification.environment_digest", self.environment_digest)?;
        require_digest("verification.required_snapshot", self.required_snapshot)?;
        require_snapshot("verification", self.required_snapshot, base_state)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectVerificationPlanV1 {
    steps: Vec<EffectVerificationStepV1>,
}

impl EffectVerificationPlanV1 {
    pub fn new(steps: Vec<EffectVerificationStepV1>) -> Result<Self, EffectIrErrorV1> {
        if steps.len() > EFFECT_IR_MAX_VERIFICATION_STEPS_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::TooManyVerificationSteps,
                format!("verification plan has {} steps", steps.len()),
            ));
        }
        reject_duplicates(&steps, "verification steps")?;
        Ok(Self { steps })
    }

    pub fn steps(&self) -> &[EffectVerificationStepV1] {
        &self.steps
    }

    fn validate(&self, base_state: DigestV1) -> Result<(), EffectIrErrorV1> {
        if self.steps.len() > EFFECT_IR_MAX_VERIFICATION_STEPS_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::TooManyVerificationSteps,
                format!("verification plan has {} steps", self.steps.len()),
            ));
        }
        reject_duplicates(&self.steps, "verification steps")?;
        for step in &self.steps {
            step.validate(base_state)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum TypedEffectOperationV1 {
    RecoverExact {
        owner: ArtifactOwnerV1,
        capability: String,
        generation: u64,
        capability_contract_digest: DigestV1,
        arguments_digest: DigestV1,
        expected_output_digest: DigestV1,
    },
    ReplaceExactFile {
        target: DigestV1,
        expected_before: DigestV1,
        replacement: DigestV1,
    },
    CopyExact {
        source: DigestV1,
        target: DigestV1,
        expected_source_digest: DigestV1,
    },
    DeterministicTransform {
        owner: ArtifactOwnerV1,
        capability: String,
        generation: u64,
        capability_contract_digest: DigestV1,
        targets: Vec<DigestV1>,
        arguments_digest: DigestV1,
        exceptions: Vec<DigestV1>,
        effect_class: EffectClass,
    },
    InvokeCapability {
        owner: ArtifactOwnerV1,
        capability: String,
        generation: u64,
        capability_contract_digest: DigestV1,
        arguments_digest: DigestV1,
        effect_class: EffectClass,
    },
    ReturnLiteral {
        bytes: Vec<u8>,
        payload_digest: DigestV1,
    },
    RawFallback,
}

impl TypedEffectOperationV1 {
    pub const fn effect_class(&self) -> EffectClass {
        match self {
            Self::RecoverExact { .. } | Self::ReturnLiteral { .. } | Self::RawFallback => {
                EffectClass::ReadOnly
            }
            Self::ReplaceExactFile { .. } | Self::CopyExact { .. } => {
                EffectClass::ReversibleMutation
            }
            Self::DeterministicTransform { effect_class, .. }
            | Self::InvokeCapability { effect_class, .. } => *effect_class,
        }
    }

    pub const fn is_raw_fallback(&self) -> bool {
        matches!(self, Self::RawFallback)
    }

    fn capability(&self) -> Option<(ArtifactOwnerV1, &str, u64, DigestV1, EffectClass)> {
        match self {
            Self::RecoverExact {
                owner,
                capability,
                generation,
                capability_contract_digest,
                ..
            } => Some((
                *owner,
                capability,
                *generation,
                *capability_contract_digest,
                EffectClass::ReadOnly,
            )),
            Self::DeterministicTransform {
                owner,
                capability,
                generation,
                capability_contract_digest,
                effect_class,
                ..
            }
            | Self::InvokeCapability {
                owner,
                capability,
                generation,
                capability_contract_digest,
                effect_class,
                ..
            } => Some((
                *owner,
                capability,
                *generation,
                *capability_contract_digest,
                *effect_class,
            )),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), EffectIrErrorV1> {
        match self {
            Self::RecoverExact {
                capability,
                generation,
                capability_contract_digest,
                arguments_digest,
                expected_output_digest,
                ..
            } => {
                validate_identity("recover capability", capability)?;
                require_generation("recover generation", *generation)?;
                require_digest(
                    "recover capability_contract_digest",
                    *capability_contract_digest,
                )?;
                require_digest("recover arguments_digest", *arguments_digest)?;
                require_digest("recover expected_output_digest", *expected_output_digest)
            }
            Self::ReplaceExactFile {
                target,
                expected_before,
                replacement,
            } => {
                require_digest("replace target", *target)?;
                require_digest("replace expected_before", *expected_before)?;
                require_digest("replace replacement", *replacement)
            }
            Self::CopyExact {
                source,
                target,
                expected_source_digest,
            } => {
                require_digest("copy source", *source)?;
                require_digest("copy target", *target)?;
                require_digest("copy expected_source_digest", *expected_source_digest)
            }
            Self::DeterministicTransform {
                capability,
                generation,
                capability_contract_digest,
                targets,
                arguments_digest,
                exceptions,
                ..
            } => {
                validate_identity("transform capability", capability)?;
                require_generation("transform generation", *generation)?;
                require_digest(
                    "transform capability_contract_digest",
                    *capability_contract_digest,
                )?;
                require_digest("transform arguments_digest", *arguments_digest)?;
                validate_sorted_digest_set(
                    targets,
                    "transform targets",
                    EFFECT_IR_MAX_REFS_PER_OPERATION_V1,
                )?;
                if targets.is_empty() {
                    return Err(EffectIrErrorV1::new(
                        EffectIrFailureCodeV1::InvalidOperation,
                        "deterministic transform requires at least one exact target",
                    ));
                }
                validate_sorted_digest_set(
                    exceptions,
                    "transform exceptions",
                    EFFECT_IR_MAX_REFS_PER_OPERATION_V1,
                )
            }
            Self::InvokeCapability {
                capability,
                generation,
                capability_contract_digest,
                arguments_digest,
                ..
            } => {
                validate_identity("invoke capability", capability)?;
                require_generation("invoke generation", *generation)?;
                require_digest(
                    "invoke capability_contract_digest",
                    *capability_contract_digest,
                )?;
                require_digest("invoke arguments_digest", *arguments_digest)
            }
            Self::ReturnLiteral {
                bytes,
                payload_digest,
            } => {
                if bytes.len() > EFFECT_IR_MAX_LITERAL_BYTES_V1 {
                    return Err(EffectIrErrorV1::new(
                        EffectIrFailureCodeV1::LiteralTooLarge,
                        format!("literal contains {} bytes", bytes.len()),
                    ));
                }
                let expected = DigestV1::from_bytes(sha256(bytes));
                if *payload_digest != expected {
                    return Err(EffectIrErrorV1::new(
                        EffectIrFailureCodeV1::LiteralDigestMismatch,
                        format!(
                            "literal digest {} does not match exact bytes {}",
                            payload_digest.to_hex(),
                            expected.to_hex()
                        ),
                    ));
                }
                Ok(())
            }
            Self::RawFallback => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectCapabilityBindingV1 {
    pub owner: ArtifactOwnerV1,
    pub capability: String,
    pub generation: u64,
    pub contract_digest: DigestV1,
    pub max_effect_class: EffectClass,
}

impl EffectCapabilityBindingV1 {
    fn validate(&self) -> Result<(), EffectIrErrorV1> {
        validate_identity("capability binding", &self.capability)?;
        require_generation("capability generation", self.generation)?;
        require_digest("capability contract_digest", self.contract_digest)
    }

    fn key(&self) -> (ArtifactOwnerV1, &str) {
        (self.owner, &self.capability)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectAdmissionV1 {
    expected_snapshot: DigestV1,
    allowed_intents: Vec<String>,
    capabilities: Vec<EffectCapabilityBindingV1>,
}

impl EffectAdmissionV1 {
    pub fn new(
        expected_snapshot: DigestV1,
        mut allowed_intents: Vec<String>,
        mut capabilities: Vec<EffectCapabilityBindingV1>,
    ) -> Result<Self, EffectIrErrorV1> {
        require_digest("admission expected_snapshot", expected_snapshot)?;
        if allowed_intents.len() > EFFECT_IR_MAX_INTENTS_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::TooManyIntents,
                format!("admission has {} intents", allowed_intents.len()),
            ));
        }
        for intent in &allowed_intents {
            validate_identity("allowed intent", intent)?;
        }
        allowed_intents.sort();
        reject_duplicates(&allowed_intents, "allowed intents")?;
        if capabilities.len() > EFFECT_IR_MAX_CAPABILITIES_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::TooManyCapabilities,
                format!("admission has {} capabilities", capabilities.len()),
            ));
        }
        for capability in &capabilities {
            capability.validate()?;
        }
        capabilities.sort_by(|left, right| left.key().cmp(&right.key()));
        if capabilities
            .windows(2)
            .any(|pair| pair[0].key() == pair[1].key())
        {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::DuplicateMember,
                "admission contains duplicate owner/capability bindings",
            ));
        }
        Ok(Self {
            expected_snapshot,
            allowed_intents,
            capabilities,
        })
    }

    pub const fn expected_snapshot(&self) -> DigestV1 {
        self.expected_snapshot
    }

    pub fn allowed_intents(&self) -> &[String] {
        &self.allowed_intents
    }

    pub fn capabilities(&self) -> &[EffectCapabilityBindingV1] {
        &self.capabilities
    }

    fn validate(&self) -> Result<(), EffectIrErrorV1> {
        require_digest("admission expected_snapshot", self.expected_snapshot)?;
        validate_sorted_strings(
            &self.allowed_intents,
            "allowed intents",
            EFFECT_IR_MAX_INTENTS_V1,
        )?;
        if self.capabilities.len() > EFFECT_IR_MAX_CAPABILITIES_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::TooManyCapabilities,
                format!("admission has {} capabilities", self.capabilities.len()),
            ));
        }
        for capability in &self.capabilities {
            capability.validate()?;
        }
        for pair in self.capabilities.windows(2) {
            if pair[0].key() == pair[1].key() {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::DuplicateMember,
                    "admission contains duplicate owner/capability bindings",
                ));
            }
            if pair[0].key() > pair[1].key() {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::NonCanonicalOrder,
                    "capability bindings are not sorted by owner and identity",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectProgramV1 {
    contract_version: u16,
    base_state: DigestV1,
    intent: String,
    targets: Vec<EffectTargetV1>,
    preconditions: Vec<EffectPredicateV1>,
    operations: Vec<TypedEffectOperationV1>,
    exceptions: Vec<EffectExceptionV1>,
    verification: EffectVerificationPlanV1,
    rollback: EffectRollbackV1,
    action_digest: DigestV1,
}

#[derive(Serialize)]
struct EffectProgramBodyV1<'a> {
    contract_version: u16,
    base_state: DigestV1,
    intent: &'a str,
    targets: &'a [EffectTargetV1],
    preconditions: &'a [EffectPredicateV1],
    operations: &'a [TypedEffectOperationV1],
    exceptions: &'a [EffectExceptionV1],
    verification: &'a EffectVerificationPlanV1,
    rollback: EffectRollbackV1,
}

impl EffectProgramV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_state: DigestV1,
        intent: impl Into<String>,
        mut targets: Vec<EffectTargetV1>,
        mut preconditions: Vec<EffectPredicateV1>,
        operations: Vec<TypedEffectOperationV1>,
        mut exceptions: Vec<EffectExceptionV1>,
        verification: EffectVerificationPlanV1,
        rollback: EffectRollbackV1,
    ) -> Result<Self, EffectIrErrorV1> {
        targets.sort();
        reject_duplicates(&targets, "effect targets")?;
        preconditions.sort();
        reject_duplicates(&preconditions, "effect preconditions")?;
        exceptions.sort();
        reject_duplicates(&exceptions, "effect exceptions")?;
        let mut program = Self {
            contract_version: EFFECT_IR_CONTRACT_VERSION_V1,
            base_state,
            intent: intent.into(),
            targets,
            preconditions,
            operations,
            exceptions,
            verification,
            rollback,
            action_digest: DigestV1::ZERO,
        };
        program.validate_body()?;
        program.action_digest = program.expected_action_digest()?;
        Ok(program)
    }

    pub const fn contract_version(&self) -> u16 {
        self.contract_version
    }

    pub const fn base_state(&self) -> DigestV1 {
        self.base_state
    }

    pub fn intent(&self) -> &str {
        &self.intent
    }

    pub fn targets(&self) -> &[EffectTargetV1] {
        &self.targets
    }

    pub fn preconditions(&self) -> &[EffectPredicateV1] {
        &self.preconditions
    }

    pub fn operations(&self) -> &[TypedEffectOperationV1] {
        &self.operations
    }

    pub fn exceptions(&self) -> &[EffectExceptionV1] {
        &self.exceptions
    }

    pub const fn verification(&self) -> &EffectVerificationPlanV1 {
        &self.verification
    }

    pub const fn rollback(&self) -> EffectRollbackV1 {
        self.rollback
    }

    pub const fn action_digest(&self) -> DigestV1 {
        self.action_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectIrErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > EFFECT_IR_MAX_CANONICAL_BYTES_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::CanonicalPayloadTooLarge,
                format!("effect program has {} canonical bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, EffectIrErrorV1> {
        if bytes.len() > EFFECT_IR_MAX_CANONICAL_BYTES_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::CanonicalPayloadTooLarge,
                format!("effect program has {} canonical bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::NonCanonicalEncoding,
                "effect program bytes are not exact canonical JSON",
            ));
        }
        let program: Self = serde_json::from_value(value).map_err(serialization_error)?;
        program.validate()?;
        Ok(program)
    }

    pub fn validate(&self) -> Result<(), EffectIrErrorV1> {
        self.validate_body()?;
        let expected = self.expected_action_digest()?;
        if self.action_digest != expected {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::ActionDigestMismatch,
                format!(
                    "action digest {} does not match canonical body {}",
                    self.action_digest.to_hex(),
                    expected.to_hex()
                ),
            ));
        }
        Ok(())
    }

    pub fn validate_against(&self, admission: &EffectAdmissionV1) -> Result<(), EffectIrErrorV1> {
        self.validate()?;
        admission.validate()?;
        require_snapshot("program base", self.base_state, admission.expected_snapshot)?;
        if admission
            .allowed_intents
            .binary_search(&self.intent)
            .is_err()
        {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::UnlistedIntent,
                format!("intent {} is not admitted", self.intent),
            ));
        }
        for operation in &self.operations {
            let Some((owner, capability, generation, contract_digest, effect_class)) =
                operation.capability()
            else {
                continue;
            };
            let binding = admission
                .capabilities
                .binary_search_by(|candidate| candidate.key().cmp(&(owner, capability)))
                .ok()
                .map(|index| &admission.capabilities[index])
                .ok_or_else(|| {
                    EffectIrErrorV1::new(
                        EffectIrFailureCodeV1::UnlistedCapability,
                        format!("capability {owner:?}/{capability} is not admitted"),
                    )
                })?;
            if generation != binding.generation {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::CapabilityGenerationMismatch,
                    format!(
                        "capability {capability} generation {generation} does not match {}",
                        binding.generation
                    ),
                ));
            }
            if contract_digest != binding.contract_digest {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::CapabilityContractMismatch,
                    format!("capability {capability} contract digest does not match admission"),
                ));
            }
            if effect_class_rank(effect_class) > effect_class_rank(binding.max_effect_class) {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::CapabilityEffectClassExceeded,
                    format!("capability {capability} exceeds its admitted effect class"),
                ));
            }
        }
        Ok(())
    }

    fn body(&self) -> EffectProgramBodyV1<'_> {
        EffectProgramBodyV1 {
            contract_version: self.contract_version,
            base_state: self.base_state,
            intent: &self.intent,
            targets: &self.targets,
            preconditions: &self.preconditions,
            operations: &self.operations,
            exceptions: &self.exceptions,
            verification: &self.verification,
            rollback: self.rollback,
        }
    }

    fn expected_action_digest(&self) -> Result<DigestV1, EffectIrErrorV1> {
        digest_body(EFFECT_IR_ACTION_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), EffectIrErrorV1> {
        if self.contract_version != EFFECT_IR_CONTRACT_VERSION_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::UnsupportedVersion,
                format!("unsupported Effect IR version {}", self.contract_version),
            ));
        }
        require_digest("program base_state", self.base_state)?;
        validate_identity("program intent", &self.intent)?;
        validate_set(
            &self.targets,
            "effect targets",
            EFFECT_IR_MAX_TARGETS_V1,
            EffectIrFailureCodeV1::TooManyTargets,
        )?;
        validate_set(
            &self.preconditions,
            "effect preconditions",
            EFFECT_IR_MAX_PRECONDITIONS_V1,
            EffectIrFailureCodeV1::TooManyPreconditions,
        )?;
        validate_set(
            &self.exceptions,
            "effect exceptions",
            EFFECT_IR_MAX_EXCEPTIONS_V1,
            EffectIrFailureCodeV1::TooManyExceptions,
        )?;
        for target in &self.targets {
            target.validate(self.base_state)?;
        }
        for predicate in &self.preconditions {
            predicate.validate(self.base_state)?;
        }
        for exception in &self.exceptions {
            exception.validate()?;
        }
        if self.operations.is_empty() {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::InvalidOperation,
                "effect program has no operations",
            ));
        }
        if self.operations.len() > EFFECT_IR_MAX_OPERATIONS_V1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::TooManyOperations,
                format!("effect program has {} operations", self.operations.len()),
            ));
        }
        for operation in &self.operations {
            operation.validate()?;
        }
        self.verification.validate(self.base_state)?;
        self.validate_relationships()?;
        self.validate_rollback()
    }

    fn validate_relationships(&self) -> Result<(), EffectIrErrorV1> {
        let mut target_ids: Vec<DigestV1> = self
            .targets
            .iter()
            .map(|target| target.target_digest)
            .collect();
        target_ids.sort();
        reject_duplicates(&target_ids, "effect target identities")?;
        let mut exception_ids: Vec<DigestV1> = self
            .exceptions
            .iter()
            .map(|exception| exception.exception_digest)
            .collect();
        exception_ids.sort();
        reject_duplicates(&exception_ids, "effect exception identities")?;
        for exception in &self.exceptions {
            if target_ids.binary_search(&exception.target_digest).is_err() {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::MissingTarget,
                    format!(
                        "exception target {} is absent from the effect target set",
                        exception.target_digest.to_hex()
                    ),
                ));
            }
        }
        for operation in &self.operations {
            match operation {
                TypedEffectOperationV1::ReplaceExactFile { target, .. } => {
                    require_member(&target_ids, *target, "replace target")?;
                }
                TypedEffectOperationV1::CopyExact { source, target, .. } => {
                    require_member(&target_ids, *source, "copy source")?;
                    require_member(&target_ids, *target, "copy target")?;
                }
                TypedEffectOperationV1::DeterministicTransform {
                    targets,
                    exceptions,
                    ..
                } => {
                    for target in targets {
                        require_member(&target_ids, *target, "transform target")?;
                    }
                    for exception in exceptions {
                        if exception_ids.binary_search(exception).is_err() {
                            return Err(EffectIrErrorV1::new(
                                EffectIrFailureCodeV1::MissingException,
                                format!(
                                    "transform exception {} is absent from the exception set",
                                    exception.to_hex()
                                ),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_rollback(&self) -> Result<(), EffectIrErrorV1> {
        let raw_count = self
            .operations
            .iter()
            .filter(|operation| operation.is_raw_fallback())
            .count();
        if raw_count > 0 {
            if self.operations.len() != 1
                || self.rollback != EffectRollbackV1::RawFallback
                || !self.targets.is_empty()
                || !self.preconditions.is_empty()
                || !self.exceptions.is_empty()
                || !self.verification.steps.is_empty()
            {
                return Err(EffectIrErrorV1::new(
                    EffectIrFailureCodeV1::RawFallbackMixed,
                    "raw fallback must be the sole operation with no typed action metadata",
                ));
            }
            return Ok(());
        }
        if self.rollback == EffectRollbackV1::RawFallback {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::RawFallbackMixed,
                "raw_fallback rollback requires the raw fallback operation",
            ));
        }
        if self.verification.steps.is_empty() {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::VerificationRequired,
                "typed effect programs require at least one verification step",
            ));
        }
        let max_effect = self
            .operations
            .iter()
            .map(|operation| effect_class_rank(operation.effect_class()))
            .max()
            .unwrap_or(0);
        if rollback_rank(self.rollback) < max_effect {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::RollbackTooWeak,
                format!(
                    "rollback {:?} is weaker than operation effect rank {max_effect}",
                    self.rollback
                ),
            ));
        }
        let mutating_count = self
            .operations
            .iter()
            .filter(|operation| effect_class_rank(operation.effect_class()) > 0)
            .count();
        if self.rollback == EffectRollbackV1::SingleAtomic && mutating_count != 1 {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::RollbackTooWeak,
                "single_atomic rollback requires exactly one mutating operation",
            ));
        }
        Ok(())
    }
}

pub fn effect_ir_contract_manifest_v1() -> Value {
    json!({
        "contract": "zerostack.effect_ir",
        "contract_version": EFFECT_IR_CONTRACT_VERSION_V1,
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "action_domain": "zerostack.effect_ir.action.v1\u{0}",
        "program_fields": [
            "contract_version", "base_state", "intent", "targets", "preconditions",
            "operations", "exceptions", "verification", "rollback", "action_digest"
        ],
        "target_fields": ["owner", "target_digest", "required_snapshot"],
        "predicate_fields": ["predicate_digest", "scope_digest", "required_snapshot"],
        "exception_fields": ["target_digest", "exception_digest"],
        "verification_step_fields": [
            "verifier_digest", "predicate_digest", "environment_digest", "required_snapshot",
            "verifier_class"
        ],
        "verifier_classes": ["exact_checker", "sound_restricted", "empirical_incomplete"],
        "operation_kinds": [
            "recover_exact", "replace_exact_file", "copy_exact", "deterministic_transform",
            "invoke_capability", "return_literal", "raw_fallback"
        ],
        "operation_fields": {
            "recover_exact": [
                "owner", "capability", "generation", "capability_contract_digest",
                "arguments_digest", "expected_output_digest"
            ],
            "replace_exact_file": ["target", "expected_before", "replacement"],
            "copy_exact": ["source", "target", "expected_source_digest"],
            "deterministic_transform": [
                "owner", "capability", "generation", "capability_contract_digest", "targets",
                "arguments_digest", "exceptions", "effect_class"
            ],
            "invoke_capability": [
                "owner", "capability", "generation", "capability_contract_digest",
                "arguments_digest", "effect_class"
            ],
            "return_literal": ["bytes", "payload_digest"],
            "raw_fallback": []
        },
        "admission_fields": ["expected_snapshot", "allowed_intents", "capabilities"],
        "capability_binding_fields": [
            "owner", "capability", "generation", "contract_digest", "max_effect_class"
        ],
        "effect_classes": [
            "read_only", "reversible_mutation", "approval_required_mutation", "irreversible"
        ],
        "rollback_values": [
            "read_only", "single_atomic", "journaled", "workspace_clone",
            "external_transaction", "raw_fallback"
        ],
        "authority_values": ["zero_stack", "fs_zero", "graph_zero", "token_zero", "pi_zero_stack"],
        "bounds": {
            "max_canonical_bytes": EFFECT_IR_MAX_CANONICAL_BYTES_V1,
            "max_operations": EFFECT_IR_MAX_OPERATIONS_V1,
            "max_targets": EFFECT_IR_MAX_TARGETS_V1,
            "max_preconditions": EFFECT_IR_MAX_PRECONDITIONS_V1,
            "max_exceptions": EFFECT_IR_MAX_EXCEPTIONS_V1,
            "max_verification_steps": EFFECT_IR_MAX_VERIFICATION_STEPS_V1,
            "max_capabilities": EFFECT_IR_MAX_CAPABILITIES_V1,
            "max_intents": EFFECT_IR_MAX_INTENTS_V1,
            "max_string_bytes": EFFECT_IR_MAX_STRING_BYTES_V1,
            "max_literal_bytes": EFFECT_IR_MAX_LITERAL_BYTES_V1,
            "max_refs_per_operation": EFFECT_IR_MAX_REFS_PER_OPERATION_V1
        },
        "invariants": [
            "action_identity_excludes_only_action_digest",
            "declared_operation_order_is_semantic",
            "targets_preconditions_exceptions_are_sorted_sets",
            "all_state_scopes_match_base_state",
            "operation_targets_and_exceptions_resolve",
            "capability_owner_generation_contract_and_effect_class_match_admission",
            "raw_fallback_is_first_class_and_never_mixed",
            "rollback_is_not_weaker_than_effect_class",
            "typed_programs_require_verification",
            "effect_programs_never_grant_execution_authority"
        ],
        "failure_codes": [
            "unsupported_version", "canonical_payload_too_large", "non_canonical_encoding",
            "serialization_failure", "invalid_identity", "zero_digest", "zero_generation",
            "duplicate_member", "non_canonical_order", "too_many_operations", "too_many_targets",
            "too_many_preconditions", "too_many_exceptions", "too_many_verification_steps",
            "too_many_capabilities", "too_many_intents", "literal_too_large",
            "literal_digest_mismatch", "action_digest_mismatch", "stale_base_state",
            "unlisted_intent", "unlisted_capability", "capability_generation_mismatch",
            "capability_contract_mismatch", "capability_effect_class_exceeded", "missing_target",
            "missing_exception", "invalid_operation", "raw_fallback_mixed", "rollback_too_weak",
            "verification_required"
        ]
    })
}

pub fn effect_ir_contract_digest_v1() -> DigestV1 {
    DigestV1::from_bytes(sha256(
        canonical_json(&effect_ir_contract_manifest_v1()).as_bytes(),
    ))
}

fn digest_body<T: Serialize>(domain: &[u8], value: &T) -> Result<DigestV1, EffectIrErrorV1> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let canonical = canonical_json(&value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Ok(DigestV1::from_bytes(sha256(&bytes)))
}

fn serialization_error(error: serde_json::Error) -> EffectIrErrorV1 {
    EffectIrErrorV1::new(
        EffectIrFailureCodeV1::SerializationFailure,
        error.to_string(),
    )
}

fn validate_identity(label: &str, value: &str) -> Result<(), EffectIrErrorV1> {
    if value.is_empty()
        || value.len() > EFFECT_IR_MAX_STRING_BYTES_V1
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
    {
        Err(EffectIrErrorV1::new(
            EffectIrFailureCodeV1::InvalidIdentity,
            format!("{label} is empty, too long, or contains a non-canonical byte"),
        ))
    } else {
        Ok(())
    }
}

fn require_digest(label: &str, digest: DigestV1) -> Result<(), EffectIrErrorV1> {
    if digest == DigestV1::ZERO {
        Err(EffectIrErrorV1::new(
            EffectIrFailureCodeV1::ZeroDigest,
            format!("{label} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

fn require_generation(label: &str, generation: u64) -> Result<(), EffectIrErrorV1> {
    if generation == 0 {
        Err(EffectIrErrorV1::new(
            EffectIrFailureCodeV1::ZeroGeneration,
            format!("{label} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

fn require_snapshot(
    label: &str,
    actual: DigestV1,
    expected: DigestV1,
) -> Result<(), EffectIrErrorV1> {
    if actual == expected {
        Ok(())
    } else {
        Err(EffectIrErrorV1::new(
            EffectIrFailureCodeV1::StaleBaseState,
            format!(
                "{label} snapshot {} does not match current {}",
                actual.to_hex(),
                expected.to_hex()
            ),
        ))
    }
}

fn reject_duplicates<T: Eq>(values: &[T], label: &str) -> Result<(), EffectIrErrorV1> {
    for left in 0..values.len() {
        if values[left + 1..].contains(&values[left]) {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
    }
    Ok(())
}

fn validate_set<T: Ord>(
    values: &[T],
    label: &str,
    max: usize,
    too_many_code: EffectIrFailureCodeV1,
) -> Result<(), EffectIrErrorV1> {
    if values.len() > max {
        return Err(EffectIrErrorV1::new(
            too_many_code,
            format!("{label} contains {} members", values.len()),
        ));
    }
    for pair in values.windows(2) {
        if pair[0] == pair[1] {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
        if pair[0] > pair[1] {
            return Err(EffectIrErrorV1::new(
                EffectIrFailureCodeV1::NonCanonicalOrder,
                format!("{label} is not strictly sorted"),
            ));
        }
    }
    Ok(())
}

fn validate_sorted_digest_set(
    values: &[DigestV1],
    label: &str,
    max: usize,
) -> Result<(), EffectIrErrorV1> {
    validate_set(values, label, max, EffectIrFailureCodeV1::InvalidOperation)?;
    for digest in values {
        require_digest(label, *digest)?;
    }
    Ok(())
}

fn validate_sorted_strings(
    values: &[String],
    label: &str,
    max: usize,
) -> Result<(), EffectIrErrorV1> {
    validate_set(values, label, max, EffectIrFailureCodeV1::TooManyIntents)?;
    for value in values {
        validate_identity(label, value)?;
    }
    Ok(())
}

fn require_member(
    sorted: &[DigestV1],
    value: DigestV1,
    label: &str,
) -> Result<(), EffectIrErrorV1> {
    if sorted.binary_search(&value).is_ok() {
        Ok(())
    } else {
        Err(EffectIrErrorV1::new(
            EffectIrFailureCodeV1::MissingTarget,
            format!("{label} {} is absent from the target set", value.to_hex()),
        ))
    }
}

const fn effect_class_rank(effect_class: EffectClass) -> u8 {
    match effect_class {
        EffectClass::ReadOnly => 0,
        EffectClass::ReversibleMutation => 1,
        EffectClass::ApprovalRequiredMutation => 2,
        EffectClass::Irreversible => 4,
    }
}

const fn rollback_rank(rollback: EffectRollbackV1) -> u8 {
    match rollback {
        EffectRollbackV1::ReadOnly => 0,
        EffectRollbackV1::SingleAtomic => 1,
        EffectRollbackV1::Journaled => 2,
        EffectRollbackV1::WorkspaceClone => 3,
        EffectRollbackV1::ExternalTransaction => 4,
        EffectRollbackV1::RawFallback => 0,
    }
}

