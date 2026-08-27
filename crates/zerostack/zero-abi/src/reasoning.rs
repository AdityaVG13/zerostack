//! Frozen same-model reasoning identity and strict no-downshift admission.
//!
//! This contract is a comparison identity. It does not infer quality from token
//! counts, cache hits, or shorter traces. Strict admission keeps semantic
//! identities and provider policy exact and permits only nondecreasing token
//! ceilings/reserves. `decoder_identity` commits resolved sampling, randomness,
//! context behavior, and provider defaults; an unresolved label is not valid authority.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Sha256Digest, canonical_json, sha256};

pub const REASONING_CONTRACT_SCHEMA_VERSION: &str = "racc-r-reasoning-contract";
pub const REASONING_CONTRACT_VERSION: u16 = 1;
pub const REASONING_CONTRACT_MAX_CANONICAL_BYTES: usize = 32 * 1024;
pub const REASONING_CONTRACT_MAX_EXTENSION_BYTES: usize = 8 * 1024;
pub const REASONING_CONTRACT_MAX_EXTENSION_NODES: usize = 256;
pub const REASONING_CONTRACT_MAX_EXTENSION_DEPTH: usize = 16;
pub const REASONING_CONTRACT_MAX_ID_BYTES: usize = 128;
pub const REASONING_CONTRACT_MAX_TOOL_PERMISSIONS: usize = 256;
pub const REASONING_CONTRACT_MAX_STOP_SEQUENCES: usize = 64;
pub const REASONING_CONTRACT_MAX_STOP_SEQUENCE_BYTES: usize = 256;
pub const REASONING_CONTRACT_TEMPERATURE_PPM_MAX: u32 = 2_000_000;
pub const REASONING_CONTRACT_TOP_P_PPM_MAX: u32 = 1_000_000;

const CONTRACT_DOMAIN: &[u8] = b"zerostack.reasoning_contract.contract\0";
const INSTANCE_DOMAIN: &[u8] = b"zerostack.reasoning_contract.instance\0";
const ADMISSION_DOMAIN: &[u8] = b"zerostack.reasoning_contract.admission\0";
const SCHEMA_DOMAIN: &[u8] = b"zerostack.reasoning_contract.schema\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeStatePolicy {
    ExactRequired,
    ExactIfAvailable,
    CleanRestart,
    ScopedCertificate,
    Unavailable,
}

/// Explicit sampling parameters (CONTRACT-002). Integer parts-per-million
/// encoding keeps canonical bytes exact -- no float canonicalization hazards.
/// Absence of a [`ReasoningContract`] field means provider defaults are
/// committed by `decoder_identity`; presence binds the override into the
/// contract identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingParams {
    /// Temperature in parts per million: `0` (greedy) ..= 2_000_000 (2.0).
    pub temperature_ppm: u32,
    /// Nucleus sampling cutoff in parts per million: `1_000_000` (1.0) means
    /// no truncation.
    pub top_p_ppm: u32,
    /// Optional fixed seed; `None` leaves seeding to the provider.
    pub seed: Option<u64>,
}

impl SamplingParams {
    pub fn new(
        temperature_ppm: u32,
        top_p_ppm: u32,
        seed: Option<u64>,
    ) -> Result<Self, ReasoningContractError> {
        let params = Self {
            temperature_ppm,
            top_p_ppm,
            seed,
        };
        params.validate()?;
        Ok(params)
    }

    pub fn validate(&self) -> Result<(), ReasoningContractError> {
        if self.temperature_ppm > REASONING_CONTRACT_TEMPERATURE_PPM_MAX {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidSamplingParams,
                "temperature_ppm exceeds 2_000_000 (2.0)",
            ));
        }
        if self.top_p_ppm == 0 || self.top_p_ppm > REASONING_CONTRACT_TOP_P_PPM_MAX {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidSamplingParams,
                "top_p_ppm must be in 1..=1_000_000",
            ));
        }
        Ok(())
    }
}

/// Explicit stopping policy (CONTRACT-002): bounded stop sequences plus an
/// optional hard step ceiling.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoppingPolicy {
    pub stop_sequences: Vec<String>,
    pub max_steps: Option<u32>,
}

impl StoppingPolicy {
    pub fn new(
        stop_sequences: Vec<String>,
        max_steps: Option<u32>,
    ) -> Result<Self, ReasoningContractError> {
        let policy = Self {
            stop_sequences,
            max_steps,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), ReasoningContractError> {
        if self.stop_sequences.len() > REASONING_CONTRACT_MAX_STOP_SEQUENCES {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidStoppingPolicy,
                "stop_sequences exceeds the 64-sequence bound",
            ));
        }
        for sequence in &self.stop_sequences {
            if sequence.is_empty()
                || sequence.len() > REASONING_CONTRACT_MAX_STOP_SEQUENCE_BYTES
                || sequence.chars().any(char::is_control)
            {
                return Err(ReasoningContractError::new(
                    ReasoningContractFailureCode::InvalidStoppingPolicy,
                    "stop sequences must be nonempty, bounded, and control-free",
                ));
            }
        }
        if self.max_steps.is_some_and(|steps| steps == 0) {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidStoppingPolicy,
                "max_steps must be nonzero when set",
            ));
        }
        Ok(())
    }
}

/// Per-tool invocation permission (CONTRACT-002): the granularity that
/// `tool_schema_digest` set-identity cannot express.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPermission {
    pub read_only: bool,
    pub approval_required: bool,
    pub max_calls: Option<u32>,
}

impl ToolPermission {
    pub fn new(
        read_only: bool,
        approval_required: bool,
        max_calls: Option<u32>,
    ) -> Result<Self, ReasoningContractError> {
        let permission = Self {
            read_only,
            approval_required,
            max_calls,
        };
        permission.validate()?;
        Ok(permission)
    }

    pub fn validate(&self) -> Result<(), ReasoningContractError> {
        if self.max_calls.is_some_and(|calls| calls == 0) {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidToolPermissions,
                "max_calls must be nonzero when set",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningContract {
    schema_version: String,
    model_identity: Sha256Digest,
    backend_identity: Sha256Digest,
    tokenizer_identity: Sha256Digest,
    decoder_identity: Sha256Digest,
    tool_schema_digest: Sha256Digest,
    reasoning_mode: String,
    reasoning_effort: String,
    max_output_tokens: u32,
    reserved_reasoning_tokens: u32,
    reserved_visible_output_tokens: u32,
    reserved_recovery_tokens: u32,
    native_state_policy: NativeStatePolicy,
    allow_effort_downshift: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    provider_extension: BTreeMap<String, Value>,
    // CONTRACT-002: explicit invocation bindings. Absence (None/empty) is a
    // legitimate declared state meaning provider defaults; presence binds the
    // override into the contract identity and strict comparison.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sampling_params: Option<SamplingParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopping_policy: Option<StoppingPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_prompt_root: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    tool_permissions: BTreeMap<String, ToolPermission>,
}

impl ReasoningContract {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_identity: Sha256Digest,
        backend_identity: Sha256Digest,
        tokenizer_identity: Sha256Digest,
        decoder_identity: Sha256Digest,
        tool_schema_digest: Sha256Digest,
        reasoning_mode: impl Into<String>,
        reasoning_effort: impl Into<String>,
        max_output_tokens: u32,
        reserved_reasoning_tokens: u32,
        reserved_visible_output_tokens: u32,
        reserved_recovery_tokens: u32,
        native_state_policy: NativeStatePolicy,
        allow_effort_downshift: bool,
        provider_extension: BTreeMap<String, Value>,
    ) -> Result<Self, ReasoningContractError> {
        let contract = Self {
            schema_version: REASONING_CONTRACT_SCHEMA_VERSION.into(),
            model_identity,
            backend_identity,
            tokenizer_identity,
            decoder_identity,
            tool_schema_digest,
            reasoning_mode: reasoning_mode.into(),
            reasoning_effort: reasoning_effort.into(),
            max_output_tokens,
            reserved_reasoning_tokens,
            reserved_visible_output_tokens,
            reserved_recovery_tokens,
            native_state_policy,
            allow_effort_downshift,
            provider_extension,
            sampling_params: None,
            stopping_policy: None,
            system_prompt_root: None,
            tool_permissions: BTreeMap::new(),
        };
        contract.validate()?;
        Ok(contract)
    }

    /// Bind the CONTRACT-002 invocation fields (sampling params, stopping
    /// policy, system prompt root, per-tool permissions) into the contract.
    /// Any binding participates in the canonical bytes, the identity digest,
    /// and strict paired comparison.
    pub fn with_invocation_bindings(
        mut self,
        sampling_params: SamplingParams,
        stopping_policy: StoppingPolicy,
        system_prompt_root: Option<Sha256Digest>,
        tool_permissions: BTreeMap<String, ToolPermission>,
    ) -> Result<Self, ReasoningContractError> {
        sampling_params.validate()?;
        stopping_policy.validate()?;
        if system_prompt_root.is_some_and(|root| root == Sha256Digest::ZERO) {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidSystemPromptRoot,
                "system_prompt_root must be nonzero when set",
            ));
        }
        validate_tool_permissions(&tool_permissions)?;
        self.sampling_params = Some(sampling_params);
        self.stopping_policy = Some(stopping_policy);
        self.system_prompt_root = system_prompt_root;
        self.tool_permissions = tool_permissions;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), ReasoningContractError> {
        if self.schema_version != REASONING_CONTRACT_SCHEMA_VERSION {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::SchemaVersionMismatch,
                "reasoning contract schema version is not v1",
            ));
        }
        for (label, digest) in [
            ("model identity", self.model_identity),
            ("backend identity", self.backend_identity),
            ("tokenizer identity", self.tokenizer_identity),
            ("decoder identity", self.decoder_identity),
            ("tool schema", self.tool_schema_digest),
        ] {
            if digest == Sha256Digest::ZERO {
                return Err(ReasoningContractError::new(
                    ReasoningContractFailureCode::MissingIdentity,
                    format!("{label} digest is zero"),
                ));
            }
        }
        validate_id("reasoning mode", &self.reasoning_mode)?;
        validate_id("reasoning effort", &self.reasoning_effort)?;
        if self.max_output_tokens == 0
            || self.reserved_visible_output_tokens > self.max_output_tokens
        {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidTokenReservation,
                "max output must be nonzero and cover the reserved visible output",
            ));
        }
        validate_provider_extension(&self.provider_extension)?;
        validate_tool_permissions(&self.tool_permissions)?;
        if let Some(params) = &self.sampling_params {
            params.validate()?;
        }
        if let Some(policy) = &self.stopping_policy {
            policy.validate()?;
        }
        if self
            .system_prompt_root
            .is_some_and(|root| root == Sha256Digest::ZERO)
        {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InvalidSystemPromptRoot,
                "system_prompt_root must be nonzero when set",
            ));
        }
        if self.canonical_bytes_unchecked()?.len() > REASONING_CONTRACT_MAX_CANONICAL_BYTES {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::CanonicalPayloadTooLarge,
                "reasoning contract exceeds its canonical byte bound",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReasoningContractError> {
        self.validate()?;
        self.canonical_bytes_unchecked()
    }

    fn canonical_bytes_unchecked(&self) -> Result<Vec<u8>, ReasoningContractError> {
        let value = serde_json::to_value(self).map_err(json_error)?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReasoningContractError> {
        if bytes.len() > REASONING_CONTRACT_MAX_CANONICAL_BYTES {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::CanonicalPayloadTooLarge,
                "reasoning contract exceeds its canonical byte bound",
            ));
        }
        let contract: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        contract.validate()?;
        if contract.canonical_bytes_unchecked()? != bytes {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::NonCanonicalEncoding,
                "reasoning contract is not canonical sorted-key JSON",
            ));
        }
        Ok(contract)
    }

    pub fn identity_digest(&self) -> Result<Sha256Digest, ReasoningContractError> {
        Ok(domain_digest(INSTANCE_DOMAIN, &self.canonical_bytes()?))
    }

    pub fn admitted_input_ceiling(
        &self,
        context_capacity: u32,
        reserved_tool_tokens: u32,
    ) -> Result<u32, ReasoningContractError> {
        self.validate()?;
        context_capacity
            .checked_sub(self.reserved_reasoning_tokens)
            .and_then(|value| value.checked_sub(self.reserved_visible_output_tokens))
            .and_then(|value| value.checked_sub(self.reserved_recovery_tokens))
            .and_then(|value| value.checked_sub(reserved_tool_tokens))
            .ok_or_else(|| {
                ReasoningContractError::new(
                    ReasoningContractFailureCode::ContextCapacityExceeded,
                    "reasoning, output, recovery, and tool reserves exceed context capacity",
                )
            })
    }

    pub fn admit_input(
        &self,
        context_capacity: u32,
        reserved_tool_tokens: u32,
        logical_input_tokens: u32,
    ) -> Result<u32, ReasoningContractError> {
        let ceiling = self.admitted_input_ceiling(context_capacity, reserved_tool_tokens)?;
        if logical_input_tokens > ceiling {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::InputExceedsHeadroom,
                "logical input consumes protected reasoning, output, tool, or recovery headroom",
            ));
        }
        Ok(ceiling - logical_input_tokens)
    }

    pub const fn model_identity(&self) -> Sha256Digest {
        self.model_identity
    }
    pub const fn backend_identity(&self) -> Sha256Digest {
        self.backend_identity
    }
    pub const fn tokenizer_identity(&self) -> Sha256Digest {
        self.tokenizer_identity
    }
    pub const fn decoder_identity(&self) -> Sha256Digest {
        self.decoder_identity
    }
    pub const fn tool_schema_digest(&self) -> Sha256Digest {
        self.tool_schema_digest
    }
    pub fn reasoning_mode(&self) -> &str {
        &self.reasoning_mode
    }
    pub fn reasoning_effort(&self) -> &str {
        &self.reasoning_effort
    }
    pub const fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }
    pub const fn reserved_reasoning_tokens(&self) -> u32 {
        self.reserved_reasoning_tokens
    }
    pub const fn reserved_visible_output_tokens(&self) -> u32 {
        self.reserved_visible_output_tokens
    }
    pub const fn reserved_recovery_tokens(&self) -> u32 {
        self.reserved_recovery_tokens
    }
    pub const fn native_state_policy(&self) -> NativeStatePolicy {
        self.native_state_policy
    }
    pub const fn allow_effort_downshift(&self) -> bool {
        self.allow_effort_downshift
    }
    pub fn provider_extension(&self) -> &BTreeMap<String, Value> {
        &self.provider_extension
    }
    pub const fn sampling_params(&self) -> Option<&SamplingParams> {
        self.sampling_params.as_ref()
    }
    pub const fn stopping_policy(&self) -> Option<&StoppingPolicy> {
        self.stopping_policy.as_ref()
    }
    pub const fn system_prompt_root(&self) -> Option<Sha256Digest> {
        self.system_prompt_root
    }
    pub fn tool_permissions(&self) -> &BTreeMap<String, ToolPermission> {
        &self.tool_permissions
    }
}

/// Opaque result of comparing two validated strict reasoning contracts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StrictReasoningAdmission {
    contract_version: u16,
    baseline_contract_digest: Sha256Digest,
    candidate_contract_digest: Sha256Digest,
    same_comparison_class: bool,
    max_output_tokens_added: u32,
    reasoning_tokens_added: u32,
    visible_output_tokens_added: u32,
    recovery_tokens_added: u32,
    admission_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StrictReasoningAdmissionRecord {
    pub contract_version: u16,
    pub baseline_contract_digest: Sha256Digest,
    pub candidate_contract_digest: Sha256Digest,
    pub same_comparison_class: bool,
    pub max_output_tokens_added: u32,
    pub reasoning_tokens_added: u32,
    pub visible_output_tokens_added: u32,
    pub recovery_tokens_added: u32,
    pub admission_digest: Sha256Digest,
}

impl StrictReasoningAdmission {
    fn new(
        baseline: &ReasoningContract,
        candidate: &ReasoningContract,
    ) -> Result<Self, ReasoningContractError> {
        let mut admission = Self {
            contract_version: REASONING_CONTRACT_VERSION,
            baseline_contract_digest: baseline.identity_digest()?,
            candidate_contract_digest: candidate.identity_digest()?,
            same_comparison_class: baseline == candidate,
            max_output_tokens_added: candidate.max_output_tokens - baseline.max_output_tokens,
            reasoning_tokens_added: candidate.reserved_reasoning_tokens
                - baseline.reserved_reasoning_tokens,
            visible_output_tokens_added: candidate.reserved_visible_output_tokens
                - baseline.reserved_visible_output_tokens,
            recovery_tokens_added: candidate.reserved_recovery_tokens
                - baseline.reserved_recovery_tokens,
            admission_digest: Sha256Digest::ZERO,
        };
        admission.admission_digest = admission.expected_digest();
        Ok(admission)
    }

    fn expected_digest(&self) -> Sha256Digest {
        admission_digest(
            self.contract_version,
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.same_comparison_class,
            AdmissionTokenAdditions {
                max_output: self.max_output_tokens_added,
                reasoning: self.reasoning_tokens_added,
                visible_output: self.visible_output_tokens_added,
                recovery: self.recovery_tokens_added,
            },
        )
    }

    pub fn validate(&self) -> Result<(), ReasoningContractError> {
        validate_admission_shape(
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.same_comparison_class,
            AdmissionTokenAdditions {
                max_output: self.max_output_tokens_added,
                reasoning: self.reasoning_tokens_added,
                visible_output: self.visible_output_tokens_added,
                recovery: self.recovery_tokens_added,
            },
        )?;
        validate_admission_fields(
            self.contract_version,
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.admission_digest,
            self.expected_digest(),
        )
    }

    pub fn record(&self) -> StrictReasoningAdmissionRecord {
        StrictReasoningAdmissionRecord {
            contract_version: self.contract_version,
            baseline_contract_digest: self.baseline_contract_digest,
            candidate_contract_digest: self.candidate_contract_digest,
            same_comparison_class: self.same_comparison_class,
            max_output_tokens_added: self.max_output_tokens_added,
            reasoning_tokens_added: self.reasoning_tokens_added,
            visible_output_tokens_added: self.visible_output_tokens_added,
            recovery_tokens_added: self.recovery_tokens_added,
            admission_digest: self.admission_digest,
        }
    }

    pub const fn baseline_contract_digest(&self) -> Sha256Digest {
        self.baseline_contract_digest
    }
    pub const fn candidate_contract_digest(&self) -> Sha256Digest {
        self.candidate_contract_digest
    }
    pub const fn same_comparison_class(&self) -> bool {
        self.same_comparison_class
    }
    pub const fn digest(&self) -> Sha256Digest {
        self.admission_digest
    }
}

impl StrictReasoningAdmissionRecord {
    pub fn validate(&self) -> Result<(), ReasoningContractError> {
        validate_admission_shape(
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.same_comparison_class,
            AdmissionTokenAdditions {
                max_output: self.max_output_tokens_added,
                reasoning: self.reasoning_tokens_added,
                visible_output: self.visible_output_tokens_added,
                recovery: self.recovery_tokens_added,
            },
        )?;
        validate_admission_fields(
            self.contract_version,
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.admission_digest,
            admission_digest(
                self.contract_version,
                self.baseline_contract_digest,
                self.candidate_contract_digest,
                self.same_comparison_class,
                AdmissionTokenAdditions {
                    max_output: self.max_output_tokens_added,
                    reasoning: self.reasoning_tokens_added,
                    visible_output: self.visible_output_tokens_added,
                    recovery: self.recovery_tokens_added,
                },
            ),
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReasoningContractError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(json_error)?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReasoningContractError> {
        if bytes.len() > REASONING_CONTRACT_MAX_CANONICAL_BYTES {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::CanonicalPayloadTooLarge,
                "strict reasoning admission exceeds its canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(ReasoningContractError::new(
                ReasoningContractFailureCode::NonCanonicalEncoding,
                "strict reasoning admission is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

pub fn verify_strict_no_downshift(
    baseline: &ReasoningContract,
    candidate: &ReasoningContract,
) -> Result<StrictReasoningAdmission, ReasoningContractError> {
    baseline.validate()?;
    candidate.validate()?;
    if baseline.allow_effort_downshift || candidate.allow_effort_downshift {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::EffortDownshiftAllowed,
            "strict and amplify contracts must set allow_effort_downshift to false",
        ));
    }
    if baseline.model_identity != candidate.model_identity
        || baseline.backend_identity != candidate.backend_identity
        || baseline.tokenizer_identity != candidate.tokenizer_identity
        || baseline.decoder_identity != candidate.decoder_identity
        || baseline.tool_schema_digest != candidate.tool_schema_digest
    {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ComparisonIdentityMismatch,
            "model, backend, tokenizer, decoder, and tool schema must remain exact",
        ));
    }
    if baseline.reasoning_mode != candidate.reasoning_mode {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ReasoningModeMismatch,
            "strict reasoning mode changed without a cross-class theorem",
        ));
    }
    if baseline.reasoning_effort != candidate.reasoning_effort {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ReasoningEffortMismatch,
            "strict reasoning effort changed without a cross-class theorem",
        ));
    }
    if baseline.native_state_policy != candidate.native_state_policy {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::NativeStatePolicyMismatch,
            "native reasoning-state policy changed without a cross-class theorem",
        ));
    }
    if baseline.provider_extension != candidate.provider_extension {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ProviderExtensionMismatch,
            "provider extension changed without a cross-class theorem",
        ));
    }
    // CONTRACT-002 invocation bindings: any mismatch reclassifies the pair,
    // exactly like the other comparison-identity fields.
    if baseline.sampling_params != candidate.sampling_params {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvocationBindingMismatch,
            "sampling parameters changed without a cross-class theorem",
        ));
    }
    if baseline.stopping_policy != candidate.stopping_policy {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvocationBindingMismatch,
            "stopping policy changed without a cross-class theorem",
        ));
    }
    if baseline.system_prompt_root != candidate.system_prompt_root {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvocationBindingMismatch,
            "system prompt root changed without a cross-class theorem",
        ));
    }
    if baseline.tool_permissions != candidate.tool_permissions {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvocationBindingMismatch,
            "per-tool permission set changed without a cross-class theorem",
        ));
    }
    if candidate.max_output_tokens < baseline.max_output_tokens {
        return Err(downshift(
            ReasoningContractFailureCode::OutputCeilingDownshift,
            "maximum output tokens",
        ));
    }
    if candidate.reserved_reasoning_tokens < baseline.reserved_reasoning_tokens {
        return Err(downshift(
            ReasoningContractFailureCode::ReasoningReserveDownshift,
            "reserved reasoning tokens",
        ));
    }
    if candidate.reserved_visible_output_tokens < baseline.reserved_visible_output_tokens {
        return Err(downshift(
            ReasoningContractFailureCode::VisibleOutputReserveDownshift,
            "reserved visible output tokens",
        ));
    }
    if candidate.reserved_recovery_tokens < baseline.reserved_recovery_tokens {
        return Err(downshift(
            ReasoningContractFailureCode::RecoveryReserveDownshift,
            "reserved recovery tokens",
        ));
    }
    StrictReasoningAdmission::new(baseline, candidate)
}

pub fn reasoning_contract_schema() -> Value {
    json!({
        "$defs": {
            "digest": {"pattern": "^[0-9a-f]{64}$", "type": "string"},
            "sampling_params": {
                "additionalProperties": false,
                "properties": {
                    "seed": {"anyOf": [{"minimum": 0, "type": "integer"}, {"type": "null"}]},
                    "temperature_ppm": {"maximum": 2000000, "minimum": 0, "type": "integer"},
                    "top_p_ppm": {"maximum": 1000000, "minimum": 1, "type": "integer"}
                },
                "required": ["temperature_ppm", "top_p_ppm"],
                "type": "object"
            },
            "stopping_policy": {
                "additionalProperties": false,
                "properties": {
                    "max_steps": {"anyOf": [{"minimum": 1, "type": "integer"}, {"type": "null"}]},
                    "stop_sequences": {
                        "items": {"maxLength": 256, "minLength": 1, "type": "string"},
                        "maxItems": 64,
                        "type": "array"
                    }
                },
                "required": ["stop_sequences"],
                "type": "object"
            },
            "tool_permission": {
                "additionalProperties": false,
                "properties": {
                    "approval_required": {"type": "boolean"},
                    "max_calls": {"anyOf": [{"minimum": 1, "type": "integer"}, {"type": "null"}]},
                    "read_only": {"type": "boolean"}
                },
                "required": ["read_only", "approval_required"],
                "type": "object"
            }
        },
        "$id": "https://zerostack.dev/schemas/racc-r/reasoning-contract-v1.json",
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "additionalProperties": false,
        "properties": {
            "allow_effort_downshift": {"type": "boolean"},
            "backend_identity": {"$ref": "#/$defs/digest"},
            "decoder_identity": {"$ref": "#/$defs/digest"},
            "max_output_tokens": {"minimum": 1, "type": "integer"},
            "model_identity": {"$ref": "#/$defs/digest"},
            "native_state_policy": {"enum": ["exact-required", "exact-if-available", "clean-restart", "scoped-certificate", "unavailable"]},
            "provider_extension": {"additionalProperties": true, "type": "object"},
            "reasoning_effort": {"minLength": 1, "type": "string"},
            "reasoning_mode": {"minLength": 1, "type": "string"},
            "reserved_reasoning_tokens": {"minimum": 0, "type": "integer"},
            "reserved_recovery_tokens": {"minimum": 0, "type": "integer"},
            "reserved_visible_output_tokens": {"minimum": 0, "type": "integer"},
            "sampling_params": {"$ref": "#/$defs/sampling_params"},
            "schema_version": {"const": REASONING_CONTRACT_SCHEMA_VERSION},
            "stopping_policy": {"$ref": "#/$defs/stopping_policy"},
            "system_prompt_root": {"$ref": "#/$defs/digest"},
            "tokenizer_identity": {"$ref": "#/$defs/digest"},
            "tool_permissions": {"additionalProperties": {"$ref": "#/$defs/tool_permission"}, "maxProperties": 256, "type": "object"},
            "tool_schema_digest": {"$ref": "#/$defs/digest"},
        },
        "required": [
            "schema_version", "model_identity", "backend_identity", "tokenizer_identity",
            "decoder_identity", "tool_schema_digest", "reasoning_mode", "reasoning_effort",
            "max_output_tokens", "reserved_reasoning_tokens", "reserved_visible_output_tokens",
            "reserved_recovery_tokens", "native_state_policy", "allow_effort_downshift",
        ],
        "title": "RACC-R ReasoningContract V1",
        "type": "object",
    })
}

pub fn reasoning_contract_manifest() -> Value {
    json!({
        "canonical_encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "comparison_identity_fields": [
            "model_identity", "backend_identity", "tokenizer_identity", "decoder_identity",
            "tool_schema_digest", "reasoning_mode", "reasoning_effort", "native_state_policy",
            "provider_extension", "sampling_params", "stopping_policy", "system_prompt_root",
            "tool_permissions",
        ],
        "contract_version": REASONING_CONTRACT_VERSION,
        "headroom_order": ["reasoning", "visible_output", "tool", "recovery", "input"],
        "max_canonical_bytes": REASONING_CONTRACT_MAX_CANONICAL_BYTES,
        "name": "zerostack.reasoning_contract",
        "negative_space": [
            "cache_eligibility_as_hit", "effort_downshift", "opaque_state_summary_as_exact",
            "reasoning_tokens_estimated_from_visible_output", "silent_output_ceiling_reduction",
            "silent_reserve_reduction", "token_reduction_as_quality",
        ],
        "schema_digest": reasoning_contract_schema_digest(),
        "strict_exact_fields": [
            "model_identity", "backend_identity", "tokenizer_identity", "decoder_identity",
            "tool_schema_digest", "reasoning_mode", "reasoning_effort", "native_state_policy",
            "provider_extension", "sampling_params", "stopping_policy", "system_prompt_root",
            "tool_permissions", "allow_effort_downshift_false",
        ],
        "strict_nondecreasing_fields": [
            "max_output_tokens", "reserved_reasoning_tokens", "reserved_visible_output_tokens",
            "reserved_recovery_tokens",
        ],
    })
}

pub fn reasoning_contract_schema_digest() -> Sha256Digest {
    domain_digest(
        SCHEMA_DOMAIN,
        canonical_json(&reasoning_contract_schema()).as_bytes(),
    )
}

pub fn reasoning_contract_digest() -> Sha256Digest {
    domain_digest(
        CONTRACT_DOMAIN,
        canonical_json(&reasoning_contract_manifest()).as_bytes(),
    )
}

#[derive(Clone, Copy)]
struct AdmissionTokenAdditions {
    max_output: u32,
    reasoning: u32,
    visible_output: u32,
    recovery: u32,
}

fn admission_digest(
    contract_version: u16,
    baseline_contract_digest: Sha256Digest,
    candidate_contract_digest: Sha256Digest,
    same_comparison_class: bool,
    added: AdmissionTokenAdditions,
) -> Sha256Digest {
    domain_digest(
        ADMISSION_DOMAIN,
        canonical_json(&json!({
            "baseline_contract_digest": baseline_contract_digest,
            "candidate_contract_digest": candidate_contract_digest,
            "contract_version": contract_version,
            "max_output_tokens_added": added.max_output,
            "reasoning_tokens_added": added.reasoning,
            "recovery_tokens_added": added.recovery,
            "same_comparison_class": same_comparison_class,
            "visible_output_tokens_added": added.visible_output,
        }))
        .as_bytes(),
    )
}

fn validate_admission_shape(
    baseline_contract_digest: Sha256Digest,
    candidate_contract_digest: Sha256Digest,
    same_comparison_class: bool,
    added: AdmissionTokenAdditions,
) -> Result<(), ReasoningContractError> {
    let all_zero = added.max_output == 0
        && added.reasoning == 0
        && added.visible_output == 0
        && added.recovery == 0;
    if same_comparison_class != (baseline_contract_digest == candidate_contract_digest)
        || same_comparison_class != all_zero
    {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvalidAdmission,
            "comparison-class flag, contract identities, and reserve deltas are inconsistent",
        ));
    }
    Ok(())
}

fn validate_admission_fields(
    contract_version: u16,
    baseline_contract_digest: Sha256Digest,
    candidate_contract_digest: Sha256Digest,
    admission_digest: Sha256Digest,
    expected_digest: Sha256Digest,
) -> Result<(), ReasoningContractError> {
    if contract_version != REASONING_CONTRACT_VERSION {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::SchemaVersionMismatch,
            "strict reasoning admission version is not v1",
        ));
    }
    if baseline_contract_digest == Sha256Digest::ZERO
        || candidate_contract_digest == Sha256Digest::ZERO
        || admission_digest == Sha256Digest::ZERO
    {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::MissingIdentity,
            "strict reasoning admission contains a zero digest",
        ));
    }
    if admission_digest != expected_digest {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::AdmissionDigestMismatch,
            "strict reasoning admission digest does not match its fields",
        ));
    }
    Ok(())
}

fn validate_id(label: &str, value: &str) -> Result<(), ReasoningContractError> {
    if value.trim().is_empty()
        || value.len() > REASONING_CONTRACT_MAX_ID_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvalidIdentifier,
            format!("{label} is empty, contains control characters, or exceeds its bound"),
        ));
    }
    Ok(())
}

fn validate_tool_permissions(
    permissions: &BTreeMap<String, ToolPermission>,
) -> Result<(), ReasoningContractError> {
    if permissions.len() > REASONING_CONTRACT_MAX_TOOL_PERMISSIONS {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::InvalidToolPermissions,
            "tool_permissions exceeds the 256-tool bound",
        ));
    }
    for (tool, permission) in permissions {
        validate_id("tool permission key", tool)?;
        permission.validate()?;
    }
    Ok(())
}

fn validate_provider_extension(
    extension: &BTreeMap<String, Value>,
) -> Result<(), ReasoningContractError> {
    let value = serde_json::to_value(extension).map_err(json_error)?;
    let bytes = canonical_json(&value);
    if bytes.len() > REASONING_CONTRACT_MAX_EXTENSION_BYTES {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ProviderExtensionTooLarge,
            "provider extension exceeds its canonical byte bound",
        ));
    }
    let mut nodes = 1usize;
    for (key, value) in extension {
        validate_id("provider extension key", key)?;
        count_value_nodes(value, 1, &mut nodes)?;
    }
    Ok(())
}

fn count_value_nodes(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), ReasoningContractError> {
    if depth > REASONING_CONTRACT_MAX_EXTENSION_DEPTH {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ProviderExtensionTooLarge,
            "provider extension exceeds its depth bound",
        ));
    }
    *nodes = nodes.checked_add(1).ok_or_else(|| {
        ReasoningContractError::new(
            ReasoningContractFailureCode::ProviderExtensionTooLarge,
            "provider extension node count overflowed",
        )
    })?;
    if *nodes > REASONING_CONTRACT_MAX_EXTENSION_NODES {
        return Err(ReasoningContractError::new(
            ReasoningContractFailureCode::ProviderExtensionTooLarge,
            "provider extension exceeds its node bound",
        ));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                count_value_nodes(value, depth + 1, nodes)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_id("provider extension key", key)?;
                count_value_nodes(value, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn domain_digest(domain: &[u8], payload: &[u8]) -> Sha256Digest {
    let mut bytes = Vec::with_capacity(domain.len() + payload.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(payload);
    Sha256Digest::from_bytes(sha256(&bytes))
}

fn json_error(error: serde_json::Error) -> ReasoningContractError {
    ReasoningContractError::new(
        ReasoningContractFailureCode::SerializationFailure,
        format!("reasoning contract JSON error: {error}"),
    )
}

fn downshift(code: ReasoningContractFailureCode, field: &str) -> ReasoningContractError {
    ReasoningContractError::new(code, format!("strict candidate lowered {field}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasoningContractFailureCode {
    SchemaVersionMismatch,
    MissingIdentity,
    InvalidIdentifier,
    InvalidTokenReservation,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    ProviderExtensionTooLarge,
    EffortDownshiftAllowed,
    ComparisonIdentityMismatch,
    ReasoningModeMismatch,
    ReasoningEffortMismatch,
    NativeStatePolicyMismatch,
    ProviderExtensionMismatch,
    InvocationBindingMismatch,
    InvalidSamplingParams,
    InvalidStoppingPolicy,
    InvalidSystemPromptRoot,
    InvalidToolPermissions,
    OutputCeilingDownshift,
    ReasoningReserveDownshift,
    VisibleOutputReserveDownshift,
    RecoveryReserveDownshift,
    ContextCapacityExceeded,
    InputExceedsHeadroom,
    AdmissionDigestMismatch,
    InvalidAdmission,
    SerializationFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningContractError {
    code: ReasoningContractFailureCode,
    detail: String,
}

impl ReasoningContractError {
    fn new(code: ReasoningContractFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> ReasoningContractFailureCode {
        self.code
    }
}

impl fmt::Display for ReasoningContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for ReasoningContractError {}
