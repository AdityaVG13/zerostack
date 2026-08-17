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

use crate::{ArtifactOwner, CwirVerifierClass, Sha256Digest, EffectClass, canonical_json, sha256};

pub const EFFECT_IR_CONTRACT_VERSION: u16 = 1;
pub const EFFECT_IR_ACTION_DOMAIN: &[u8] = b"zerostack.effect_ir.action\0";
pub const EFFECT_IR_MAX_CANONICAL_BYTES: usize = 1_048_576;
pub const EFFECT_IR_MAX_OPERATIONS: usize = 256;
pub const EFFECT_IR_MAX_TARGETS: usize = 512;
pub const EFFECT_IR_MAX_PRECONDITIONS: usize = 512;
pub const EFFECT_IR_MAX_EXCEPTIONS: usize = 512;
pub const EFFECT_IR_MAX_VERIFICATION_STEPS: usize = 128;
pub const EFFECT_IR_MAX_CAPABILITIES: usize = 512;
pub const EFFECT_IR_MAX_INTENTS: usize = 256;
pub const EFFECT_IR_MAX_STRING_BYTES: usize = 256;
pub const EFFECT_IR_MAX_LITERAL_BYTES: usize = 65_536;
pub const EFFECT_IR_MAX_REFS_PER_OPERATION: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectIrFailureCode {
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
pub struct EffectIrError {
    pub code: EffectIrFailureCode,
    pub detail: String,
}

impl EffectIrError {
    pub fn new(code: EffectIrFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> EffectIrFailureCode {
        self.code
    }
}

impl fmt::Display for EffectIrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for EffectIrError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectRollback {
    ReadOnly,
    SingleAtomic,
    Journaled,
    WorkspaceClone,
    ExternalTransaction,
    RawFallback,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectTarget {
    pub owner: ArtifactOwner,
    pub target_digest: Sha256Digest,
    pub required_snapshot: Sha256Digest,
}

impl EffectTarget {
    fn validate(self, base_state: Sha256Digest) -> Result<(), EffectIrError> {
        require_digest("target_digest", self.target_digest)?;
        require_digest("target.required_snapshot", self.required_snapshot)?;
        require_snapshot("target", self.required_snapshot, base_state)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectPredicate {
    pub predicate_digest: Sha256Digest,
    pub scope_digest: Sha256Digest,
    pub required_snapshot: Sha256Digest,
}

impl EffectPredicate {
    fn validate(self, base_state: Sha256Digest) -> Result<(), EffectIrError> {
        require_digest("predicate_digest", self.predicate_digest)?;
        require_digest("predicate.scope_digest", self.scope_digest)?;
        require_digest("predicate.required_snapshot", self.required_snapshot)?;
        require_snapshot("predicate", self.required_snapshot, base_state)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectException {
    pub target_digest: Sha256Digest,
    pub exception_digest: Sha256Digest,
}

impl EffectException {
    fn validate(self) -> Result<(), EffectIrError> {
        require_digest("exception.target_digest", self.target_digest)?;
        require_digest("exception.exception_digest", self.exception_digest)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectVerificationStep {
    pub verifier_digest: Sha256Digest,
    pub predicate_digest: Sha256Digest,
    pub environment_digest: Sha256Digest,
    pub required_snapshot: Sha256Digest,
    pub verifier_class: CwirVerifierClass,
}

impl EffectVerificationStep {
    fn validate(self, base_state: Sha256Digest) -> Result<(), EffectIrError> {
        require_digest("verification.verifier_digest", self.verifier_digest)?;
        require_digest("verification.predicate_digest", self.predicate_digest)?;
        require_digest("verification.environment_digest", self.environment_digest)?;
        require_digest("verification.required_snapshot", self.required_snapshot)?;
        require_snapshot("verification", self.required_snapshot, base_state)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectVerificationPlan {
    steps: Vec<EffectVerificationStep>,
}

impl EffectVerificationPlan {
    pub fn new(steps: Vec<EffectVerificationStep>) -> Result<Self, EffectIrError> {
        if steps.len() > EFFECT_IR_MAX_VERIFICATION_STEPS {
            return Err(EffectIrError::new(
                EffectIrFailureCode::TooManyVerificationSteps,
                format!("verification plan has {} steps", steps.len()),
            ));
        }
        reject_duplicates(&steps, "verification steps")?;
        Ok(Self { steps })
    }

    pub fn steps(&self) -> &[EffectVerificationStep] {
        &self.steps
    }

    fn validate(&self, base_state: Sha256Digest) -> Result<(), EffectIrError> {
        if self.steps.len() > EFFECT_IR_MAX_VERIFICATION_STEPS {
            return Err(EffectIrError::new(
                EffectIrFailureCode::TooManyVerificationSteps,
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
pub enum TypedEffectOperation {
    RecoverExact {
        owner: ArtifactOwner,
        capability: String,
        generation: u64,
        capability_contract_digest: Sha256Digest,
        arguments_digest: Sha256Digest,
        expected_output_digest: Sha256Digest,
    },
    ReplaceExactFile {
        target: Sha256Digest,
        expected_before: Sha256Digest,
        replacement: Sha256Digest,
    },
    CopyExact {
        source: Sha256Digest,
        target: Sha256Digest,
        expected_source_digest: Sha256Digest,
    },
    DeterministicTransform {
        owner: ArtifactOwner,
        capability: String,
        generation: u64,
        capability_contract_digest: Sha256Digest,
        targets: Vec<Sha256Digest>,
        arguments_digest: Sha256Digest,
        exceptions: Vec<Sha256Digest>,
        effect_class: EffectClass,
    },
    InvokeCapability {
        owner: ArtifactOwner,
        capability: String,
        generation: u64,
        capability_contract_digest: Sha256Digest,
        arguments_digest: Sha256Digest,
        effect_class: EffectClass,
    },
    ReturnLiteral {
        bytes: Vec<u8>,
        payload_digest: Sha256Digest,
    },
    RawFallback,
}

impl TypedEffectOperation {
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

    fn capability(&self) -> Option<(ArtifactOwner, &str, u64, Sha256Digest, EffectClass)> {
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

    fn validate(&self) -> Result<(), EffectIrError> {
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
                    EFFECT_IR_MAX_REFS_PER_OPERATION,
                )?;
                if targets.is_empty() {
                    return Err(EffectIrError::new(
                        EffectIrFailureCode::InvalidOperation,
                        "deterministic transform requires at least one exact target",
                    ));
                }
                validate_sorted_digest_set(
                    exceptions,
                    "transform exceptions",
                    EFFECT_IR_MAX_REFS_PER_OPERATION,
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
                if bytes.len() > EFFECT_IR_MAX_LITERAL_BYTES {
                    return Err(EffectIrError::new(
                        EffectIrFailureCode::LiteralTooLarge,
                        format!("literal contains {} bytes", bytes.len()),
                    ));
                }
                let expected = Sha256Digest::from_bytes(sha256(bytes));
                if *payload_digest != expected {
                    return Err(EffectIrError::new(
                        EffectIrFailureCode::LiteralDigestMismatch,
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
pub struct EffectCapabilityBinding {
    pub owner: ArtifactOwner,
    pub capability: String,
    pub generation: u64,
    pub contract_digest: Sha256Digest,
    pub max_effect_class: EffectClass,
}

impl EffectCapabilityBinding {
    fn validate(&self) -> Result<(), EffectIrError> {
        validate_identity("capability binding", &self.capability)?;
        require_generation("capability generation", self.generation)?;
        require_digest("capability contract_digest", self.contract_digest)
    }

    fn key(&self) -> (ArtifactOwner, &str) {
        (self.owner, &self.capability)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectAdmission {
    expected_snapshot: Sha256Digest,
    allowed_intents: Vec<String>,
    capabilities: Vec<EffectCapabilityBinding>,
}

impl EffectAdmission {
    pub fn new(
        expected_snapshot: Sha256Digest,
        mut allowed_intents: Vec<String>,
        mut capabilities: Vec<EffectCapabilityBinding>,
    ) -> Result<Self, EffectIrError> {
        require_digest("admission expected_snapshot", expected_snapshot)?;
        if allowed_intents.len() > EFFECT_IR_MAX_INTENTS {
            return Err(EffectIrError::new(
                EffectIrFailureCode::TooManyIntents,
                format!("admission has {} intents", allowed_intents.len()),
            ));
        }
        for intent in &allowed_intents {
            validate_identity("allowed intent", intent)?;
        }
        allowed_intents.sort();
        reject_duplicates(&allowed_intents, "allowed intents")?;
        if capabilities.len() > EFFECT_IR_MAX_CAPABILITIES {
            return Err(EffectIrError::new(
                EffectIrFailureCode::TooManyCapabilities,
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
            return Err(EffectIrError::new(
                EffectIrFailureCode::DuplicateMember,
                "admission contains duplicate owner/capability bindings",
            ));
        }
        Ok(Self {
            expected_snapshot,
            allowed_intents,
            capabilities,
        })
    }

    pub const fn expected_snapshot(&self) -> Sha256Digest {
        self.expected_snapshot
    }

    pub fn allowed_intents(&self) -> &[String] {
        &self.allowed_intents
    }

    pub fn capabilities(&self) -> &[EffectCapabilityBinding] {
        &self.capabilities
    }

    fn validate(&self) -> Result<(), EffectIrError> {
        require_digest("admission expected_snapshot", self.expected_snapshot)?;
        validate_sorted_strings(
            &self.allowed_intents,
            "allowed intents",
            EFFECT_IR_MAX_INTENTS,
        )?;
        if self.capabilities.len() > EFFECT_IR_MAX_CAPABILITIES {
            return Err(EffectIrError::new(
                EffectIrFailureCode::TooManyCapabilities,
                format!("admission has {} capabilities", self.capabilities.len()),
            ));
        }
        for capability in &self.capabilities {
            capability.validate()?;
        }
        for pair in self.capabilities.windows(2) {
            if pair[0].key() == pair[1].key() {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::DuplicateMember,
                    "admission contains duplicate owner/capability bindings",
                ));
            }
            if pair[0].key() > pair[1].key() {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::NonCanonicalOrder,
                    "capability bindings are not sorted by owner and identity",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectProgram {
    contract_version: u16,
    base_state: Sha256Digest,
    intent: String,
    targets: Vec<EffectTarget>,
    preconditions: Vec<EffectPredicate>,
    operations: Vec<TypedEffectOperation>,
    exceptions: Vec<EffectException>,
    verification: EffectVerificationPlan,
    rollback: EffectRollback,
    action_digest: Sha256Digest,
}

#[derive(Serialize)]
struct EffectProgramBody<'a> {
    contract_version: u16,
    base_state: Sha256Digest,
    intent: &'a str,
    targets: &'a [EffectTarget],
    preconditions: &'a [EffectPredicate],
    operations: &'a [TypedEffectOperation],
    exceptions: &'a [EffectException],
    verification: &'a EffectVerificationPlan,
    rollback: EffectRollback,
}

impl EffectProgram {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_state: Sha256Digest,
        intent: impl Into<String>,
        mut targets: Vec<EffectTarget>,
        mut preconditions: Vec<EffectPredicate>,
        operations: Vec<TypedEffectOperation>,
        mut exceptions: Vec<EffectException>,
        verification: EffectVerificationPlan,
        rollback: EffectRollback,
    ) -> Result<Self, EffectIrError> {
        targets.sort();
        reject_duplicates(&targets, "effect targets")?;
        preconditions.sort();
        reject_duplicates(&preconditions, "effect preconditions")?;
        exceptions.sort();
        reject_duplicates(&exceptions, "effect exceptions")?;
        let mut program = Self {
            contract_version: EFFECT_IR_CONTRACT_VERSION,
            base_state,
            intent: intent.into(),
            targets,
            preconditions,
            operations,
            exceptions,
            verification,
            rollback,
            action_digest: Sha256Digest::ZERO,
        };
        program.validate_body()?;
        program.action_digest = program.expected_action_digest()?;
        Ok(program)
    }

    pub const fn contract_version(&self) -> u16 {
        self.contract_version
    }

    pub const fn base_state(&self) -> Sha256Digest {
        self.base_state
    }

    pub fn intent(&self) -> &str {
        &self.intent
    }

    pub fn targets(&self) -> &[EffectTarget] {
        &self.targets
    }

    pub fn preconditions(&self) -> &[EffectPredicate] {
        &self.preconditions
    }

    pub fn operations(&self) -> &[TypedEffectOperation] {
        &self.operations
    }

    pub fn exceptions(&self) -> &[EffectException] {
        &self.exceptions
    }

    pub const fn verification(&self) -> &EffectVerificationPlan {
        &self.verification
    }

    pub const fn rollback(&self) -> EffectRollback {
        self.rollback
    }

    pub const fn action_digest(&self) -> Sha256Digest {
        self.action_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectIrError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > EFFECT_IR_MAX_CANONICAL_BYTES {
            return Err(EffectIrError::new(
                EffectIrFailureCode::CanonicalPayloadTooLarge,
                format!("effect program has {} canonical bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, EffectIrError> {
        if bytes.len() > EFFECT_IR_MAX_CANONICAL_BYTES {
            return Err(EffectIrError::new(
                EffectIrFailureCode::CanonicalPayloadTooLarge,
                format!("effect program has {} canonical bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(EffectIrError::new(
                EffectIrFailureCode::NonCanonicalEncoding,
                "effect program bytes are not exact canonical JSON",
            ));
        }
        let program: Self = serde_json::from_value(value).map_err(serialization_error)?;
        program.validate()?;
        Ok(program)
    }

    pub fn validate(&self) -> Result<(), EffectIrError> {
        self.validate_body()?;
        let expected = self.expected_action_digest()?;
        if self.action_digest != expected {
            return Err(EffectIrError::new(
                EffectIrFailureCode::ActionDigestMismatch,
                format!(
                    "action digest {} does not match canonical body {}",
                    self.action_digest.to_hex(),
                    expected.to_hex()
                ),
            ));
        }
        Ok(())
    }

    pub fn validate_against(&self, admission: &EffectAdmission) -> Result<(), EffectIrError> {
        self.validate()?;
        admission.validate()?;
        require_snapshot("program base", self.base_state, admission.expected_snapshot)?;
        if admission
            .allowed_intents
            .binary_search(&self.intent)
            .is_err()
        {
            return Err(EffectIrError::new(
                EffectIrFailureCode::UnlistedIntent,
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
                    EffectIrError::new(
                        EffectIrFailureCode::UnlistedCapability,
                        format!("capability {owner:?}/{capability} is not admitted"),
                    )
                })?;
            if generation != binding.generation {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::CapabilityGenerationMismatch,
                    format!(
                        "capability {capability} generation {generation} does not match {}",
                        binding.generation
                    ),
                ));
            }
            if contract_digest != binding.contract_digest {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::CapabilityContractMismatch,
                    format!("capability {capability} contract digest does not match admission"),
                ));
            }
            if effect_class_rank(effect_class) > effect_class_rank(binding.max_effect_class) {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::CapabilityEffectClassExceeded,
                    format!("capability {capability} exceeds its admitted effect class"),
                ));
            }
        }
        Ok(())
    }

    fn body(&self) -> EffectProgramBody<'_> {
        EffectProgramBody {
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

    fn expected_action_digest(&self) -> Result<Sha256Digest, EffectIrError> {
        digest_body(EFFECT_IR_ACTION_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), EffectIrError> {
        if self.contract_version != EFFECT_IR_CONTRACT_VERSION {
            return Err(EffectIrError::new(
                EffectIrFailureCode::UnsupportedVersion,
                format!("unsupported Effect IR version {}", self.contract_version),
            ));
        }
        require_digest("program base_state", self.base_state)?;
        validate_identity("program intent", &self.intent)?;
        validate_set(
            &self.targets,
            "effect targets",
            EFFECT_IR_MAX_TARGETS,
            EffectIrFailureCode::TooManyTargets,
        )?;
        validate_set(
            &self.preconditions,
            "effect preconditions",
            EFFECT_IR_MAX_PRECONDITIONS,
            EffectIrFailureCode::TooManyPreconditions,
        )?;
        validate_set(
            &self.exceptions,
            "effect exceptions",
            EFFECT_IR_MAX_EXCEPTIONS,
            EffectIrFailureCode::TooManyExceptions,
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
            return Err(EffectIrError::new(
                EffectIrFailureCode::InvalidOperation,
                "effect program has no operations",
            ));
        }
        if self.operations.len() > EFFECT_IR_MAX_OPERATIONS {
            return Err(EffectIrError::new(
                EffectIrFailureCode::TooManyOperations,
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

    fn validate_relationships(&self) -> Result<(), EffectIrError> {
        let mut target_ids: Vec<Sha256Digest> = self
            .targets
            .iter()
            .map(|target| target.target_digest)
            .collect();
        target_ids.sort();
        reject_duplicates(&target_ids, "effect target identities")?;
        let mut exception_ids: Vec<Sha256Digest> = self
            .exceptions
            .iter()
            .map(|exception| exception.exception_digest)
            .collect();
        exception_ids.sort();
        reject_duplicates(&exception_ids, "effect exception identities")?;
        for exception in &self.exceptions {
            if target_ids.binary_search(&exception.target_digest).is_err() {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::MissingTarget,
                    format!(
                        "exception target {} is absent from the effect target set",
                        exception.target_digest.to_hex()
                    ),
                ));
            }
        }
        for operation in &self.operations {
            match operation {
                TypedEffectOperation::ReplaceExactFile { target, .. } => {
                    require_member(&target_ids, *target, "replace target")?;
                }
                TypedEffectOperation::CopyExact { source, target, .. } => {
                    require_member(&target_ids, *source, "copy source")?;
                    require_member(&target_ids, *target, "copy target")?;
                }
                TypedEffectOperation::DeterministicTransform {
                    targets,
                    exceptions,
                    ..
                } => {
                    for target in targets {
                        require_member(&target_ids, *target, "transform target")?;
                    }
                    for exception in exceptions {
                        if exception_ids.binary_search(exception).is_err() {
                            return Err(EffectIrError::new(
                                EffectIrFailureCode::MissingException,
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

    fn validate_rollback(&self) -> Result<(), EffectIrError> {
        let raw_count = self
            .operations
            .iter()
            .filter(|operation| operation.is_raw_fallback())
            .count();
        if raw_count > 0 {
            if self.operations.len() != 1
                || self.rollback != EffectRollback::RawFallback
                || !self.targets.is_empty()
                || !self.preconditions.is_empty()
                || !self.exceptions.is_empty()
                || !self.verification.steps.is_empty()
            {
                return Err(EffectIrError::new(
                    EffectIrFailureCode::RawFallbackMixed,
                    "raw fallback must be the sole operation with no typed action metadata",
                ));
            }
            return Ok(());
        }
        if self.rollback == EffectRollback::RawFallback {
            return Err(EffectIrError::new(
                EffectIrFailureCode::RawFallbackMixed,
                "raw_fallback rollback requires the raw fallback operation",
            ));
        }
        if self.verification.steps.is_empty() {
            return Err(EffectIrError::new(
                EffectIrFailureCode::VerificationRequired,
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
            return Err(EffectIrError::new(
                EffectIrFailureCode::RollbackTooWeak,
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
        if self.rollback == EffectRollback::SingleAtomic && mutating_count != 1 {
            return Err(EffectIrError::new(
                EffectIrFailureCode::RollbackTooWeak,
                "single_atomic rollback requires exactly one mutating operation",
            ));
        }
        Ok(())
    }
}

pub fn effect_ir_contract_manifest() -> Value {
    json!({
        "contract": "zerostack.effect_ir",
        "contract_version": EFFECT_IR_CONTRACT_VERSION,
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "action_domain": "zerostack.effect_ir.action\u{0}",
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
            "max_canonical_bytes": EFFECT_IR_MAX_CANONICAL_BYTES,
            "max_operations": EFFECT_IR_MAX_OPERATIONS,
            "max_targets": EFFECT_IR_MAX_TARGETS,
            "max_preconditions": EFFECT_IR_MAX_PRECONDITIONS,
            "max_exceptions": EFFECT_IR_MAX_EXCEPTIONS,
            "max_verification_steps": EFFECT_IR_MAX_VERIFICATION_STEPS,
            "max_capabilities": EFFECT_IR_MAX_CAPABILITIES,
            "max_intents": EFFECT_IR_MAX_INTENTS,
            "max_string_bytes": EFFECT_IR_MAX_STRING_BYTES,
            "max_literal_bytes": EFFECT_IR_MAX_LITERAL_BYTES,
            "max_refs_per_operation": EFFECT_IR_MAX_REFS_PER_OPERATION
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

pub fn effect_ir_contract_digest() -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(
        canonical_json(&effect_ir_contract_manifest()).as_bytes(),
    ))
}

fn digest_body<T: Serialize>(domain: &[u8], value: &T) -> Result<Sha256Digest, EffectIrError> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let canonical = canonical_json(&value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Ok(Sha256Digest::from_bytes(sha256(&bytes)))
}

fn serialization_error(error: serde_json::Error) -> EffectIrError {
    EffectIrError::new(
        EffectIrFailureCode::SerializationFailure,
        error.to_string(),
    )
}

fn validate_identity(label: &str, value: &str) -> Result<(), EffectIrError> {
    if value.is_empty()
        || value.len() > EFFECT_IR_MAX_STRING_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
    {
        Err(EffectIrError::new(
            EffectIrFailureCode::InvalidIdentity,
            format!("{label} is empty, too long, or contains a non-canonical byte"),
        ))
    } else {
        Ok(())
    }
}

fn require_digest(label: &str, digest: Sha256Digest) -> Result<(), EffectIrError> {
    if digest == Sha256Digest::ZERO {
        Err(EffectIrError::new(
            EffectIrFailureCode::ZeroDigest,
            format!("{label} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

fn require_generation(label: &str, generation: u64) -> Result<(), EffectIrError> {
    if generation == 0 {
        Err(EffectIrError::new(
            EffectIrFailureCode::ZeroGeneration,
            format!("{label} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

fn require_snapshot(
    label: &str,
    actual: Sha256Digest,
    expected: Sha256Digest,
) -> Result<(), EffectIrError> {
    if actual == expected {
        Ok(())
    } else {
        Err(EffectIrError::new(
            EffectIrFailureCode::StaleBaseState,
            format!(
                "{label} snapshot {} does not match current {}",
                actual.to_hex(),
                expected.to_hex()
            ),
        ))
    }
}

fn reject_duplicates<T: Eq>(values: &[T], label: &str) -> Result<(), EffectIrError> {
    for left in 0..values.len() {
        if values[left + 1..].contains(&values[left]) {
            return Err(EffectIrError::new(
                EffectIrFailureCode::DuplicateMember,
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
    too_many_code: EffectIrFailureCode,
) -> Result<(), EffectIrError> {
    if values.len() > max {
        return Err(EffectIrError::new(
            too_many_code,
            format!("{label} contains {} members", values.len()),
        ));
    }
    for pair in values.windows(2) {
        if pair[0] == pair[1] {
            return Err(EffectIrError::new(
                EffectIrFailureCode::DuplicateMember,
                format!("{label} contains a duplicate member"),
            ));
        }
        if pair[0] > pair[1] {
            return Err(EffectIrError::new(
                EffectIrFailureCode::NonCanonicalOrder,
                format!("{label} is not strictly sorted"),
            ));
        }
    }
    Ok(())
}

fn validate_sorted_digest_set(
    values: &[Sha256Digest],
    label: &str,
    max: usize,
) -> Result<(), EffectIrError> {
    validate_set(values, label, max, EffectIrFailureCode::InvalidOperation)?;
    for digest in values {
        require_digest(label, *digest)?;
    }
    Ok(())
}

fn validate_sorted_strings(
    values: &[String],
    label: &str,
    max: usize,
) -> Result<(), EffectIrError> {
    validate_set(values, label, max, EffectIrFailureCode::TooManyIntents)?;
    for value in values {
        validate_identity(label, value)?;
    }
    Ok(())
}

fn require_member(
    sorted: &[Sha256Digest],
    value: Sha256Digest,
    label: &str,
) -> Result<(), EffectIrError> {
    if sorted.binary_search(&value).is_ok() {
        Ok(())
    } else {
        Err(EffectIrError::new(
            EffectIrFailureCode::MissingTarget,
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

const fn rollback_rank(rollback: EffectRollback) -> u8 {
    match rollback {
        EffectRollback::ReadOnly => 0,
        EffectRollback::SingleAtomic => 1,
        EffectRollback::Journaled => 2,
        EffectRollback::WorkspaceClone => 3,
        EffectRollback::ExternalTransaction => 4,
        EffectRollback::RawFallback => 0,
    }
}

